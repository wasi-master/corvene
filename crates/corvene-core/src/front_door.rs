//! Corvene (`530-front-door-credentials`, desktop#8783): GitHub Enterprise
//! behind a reverse proxy that asks for HTTP Basic credentials of its own.
//!
//! GHD 3.6.6 sends only the token (`app/src/lib/http.ts` `getAPIHeaders`),
//! so the proxy's 401 is a failed request, and git asks for a username and
//! password that end up in place of the token. Corvene asks for the proxy's
//! username and password when a request of its own gets the proxy's
//! challenge (`corvene_platform::front_door`: a 401 `Basic` challenge
//! without GitHub's `X-GitHub-Request-Id`), with the proxy dialog
//! ([`Popup::ProxyAuthentication`] with `server`), keeps the password in
//! the keychain and the username in the store (`front_door_logins`, by
//! server `host:port`). From then on Corvene's requests and git's carry them
//! next to the token, never instead of it: `Proxy-Authorization` and the
//! session cookies the proxy hands out for them
//! (`corvene_platform::front_door`, `corvene_git::set_extra_headers`). A
//! prompt cancelled once stays quiet until Corvene starts again.

use std::collections::HashMap;
use std::sync::OnceLock;

use corvene_platform::front_door::AuthRequired;
use tracing::{info, warn};

use crate::dispatcher::Dispatcher;
use crate::host::{AsyncCtx, Host};
use crate::persistence::StoreExt;
use crate::remote::spawn_bg;
use crate::state::Popup;

static AUTH_REQUESTS: OnceLock<async_channel::Sender<AuthRequired>> = OnceLock::new();

/// `corvene_platform::front_door`'s handler: any thread, so through a
/// channel.
fn on_auth_required(request: AuthRequired) {
    if let Some(tx) = AUTH_REQUESTS.get() {
        let _ = tx.try_send(request);
    }
}

impl Dispatcher {
    /// At launch: the front doors' challenges come to [`Self::front_door_auth_required`],
    /// and the saved credentials are loaded.
    pub fn start_front_doors(cx: &mut dyn Host) {
        let (tx, rx) = async_channel::unbounded::<AuthRequired>();
        if AUTH_REQUESTS.set(tx).is_ok() {
            corvene_platform::front_door::set_auth_required_handler(Some(on_auth_required));
            cx.spawn(async move |cx: &mut AsyncCtx| {
                while let Ok(request) = rx.recv().await {
                    cx.update(move |cx| {
                        Self::front_door_auth_required(request.server, request.rejected, cx)
                    });
                }
            })
            .detach();
        }
        Self::sync_front_door_credentials(cx);
    }

    /// `530-front-door-credentials` on: Corvene's requests and git get the
    /// saved credentials (read from the keychain off the main thread). Off:
    /// both forget them.
    pub fn sync_front_door_credentials(cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::FRONT_DOOR_CREDENTIALS) {
            corvene_platform::front_door::load(None);
            corvene_git::set_extra_headers(None);
            return;
        }
        let logins = s.proxy.front_doors.clone();
        spawn_bg(
            cx,
            move || {
                logins
                    .into_iter()
                    .filter_map(|(server, user)| {
                        match corvene_platform::keychain::front_door_password(&server, &user) {
                            Ok(Some(password)) => Some((server, (user, password))),
                            Ok(None) => None,
                            Err(err) => {
                                warn!(server, %err, "could not read the front door password");
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
                    .bool(crate::flags::ids::FRONT_DOOR_CREDENTIALS)
                {
                    corvene_platform::front_door::load(Some(credentials));
                    corvene_git::set_extra_headers(Some(corvene_platform::front_door::git_headers));
                }
            },
        );
    }

    /// The proxy in front of `server` (`host:port`) answered 401: ask for its
    /// username and password, unless that prompt is open or was cancelled.
    pub fn front_door_auth_required(server: String, rejected: bool, cx: &mut dyn Host) {
        let s = Self::state(cx).read(cx);
        if !s.flags.bool(crate::flags::ids::FRONT_DOOR_CREDENTIALS)
            || s.proxy.dismissed_front_doors.contains(&server)
        {
            return;
        }
        let open = s.popups.all_popups().iter().any(|p| {
            matches!(&p.popup, Popup::ProxyAuthentication { proxy, server: true, .. } if *proxy == server)
        });
        if open {
            return;
        }
        let username = s.proxy.front_doors.get(&server).cloned();
        Self::show_popup(
            Popup::ProxyAuthentication {
                proxy: server,
                server: true,
                username,
                rejected,
                retry: crate::proxy::ProxyRetry::None,
            },
            cx,
        );
    }

    /// `ProxyAuthentication` (`server`) › Save: the keychain (off the main
    /// thread), the store, then Corvene's requests and git use them.
    pub fn save_front_door_credentials(
        server: String,
        username: String,
        password: String,
        cx: &mut dyn Host,
    ) {
        let previous = Self::state(cx)
            .read(cx)
            .proxy
            .front_doors
            .get(&server)
            .cloned();
        Self::close_popup_if(
            |p| matches!(p, Popup::ProxyAuthentication { server: true, .. }),
            cx,
        );
        let stored = (server.clone(), username.clone(), password.clone());
        spawn_bg(
            cx,
            move || {
                let (server, username, password) = stored;
                corvene_platform::keychain::store_front_door_password(
                    &server, &username, &password,
                )?;
                if let Some(previous) = previous.filter(|p| *p != username) {
                    let _ =
                        corvene_platform::keychain::delete_front_door_password(&server, &previous);
                }
                Ok::<_, corvene_platform::keychain::KeychainError>(())
            },
            move |result, cx| {
                if let Err(err) = result {
                    Self::show_error("Could not save the password", err.to_string(), cx);
                    return;
                }
                Self::state(cx).update(cx, |s, cx| {
                    s.proxy.front_doors.insert(server.clone(), username.clone());
                    if let Err(err) = s.store.save_front_door_logins(&s.proxy.front_doors) {
                        warn!(%err, "could not save the front door logins");
                    }
                    cx.notify();
                });
                corvene_platform::front_door::set(&server, Some((username, password)));
                info!(server, "front door credentials saved");
            },
        );
    }

    /// `ProxyAuthentication` (`server`) › Cancel: no more prompts for this
    /// server until Corvene starts again.
    pub fn dismiss_front_door_credentials(server: String, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            s.proxy.dismissed_front_doors.insert(server);
        });
        Self::close_popup_if(
            |p| matches!(p, Popup::ProxyAuthentication { server: true, .. }),
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// Sign-in, API calls, clone, fetch and push through a reverse proxy
    /// that wants Basic credentials of its own:
    /// `packaging/linux/front-door-test.sh` starts one (in front of a stub
    /// GitHub Enterprise API and git server) and sets
    /// `CORVENE_FRONT_DOOR_TEST` to its address. Without the saved
    /// credentials every step must fail with the proxy's challenge; with them
    /// every step must work, the token untouched.
    #[test]
    fn requests_and_git_get_through_a_front_door() {
        let Ok(base) = std::env::var("CORVENE_FRONT_DOOR_TEST") else {
            return;
        };
        let scratch = PathBuf::from(std::env::var("CORVENE_FRONT_DOOR_SCRATCH").unwrap());
        let endpoint = corvene_github::endpoint::Endpoint::from_api_base(&format!("{base}/api/v3"));
        let server = corvene_platform::front_door::server_key(&base.parse().unwrap()).unwrap();
        let git = Arc::new(corvene_git::find_git().unwrap());
        let askpass = || corvene_git::AskpassEnv {
            program: scratch.join("askpass.sh"),
            logins: String::new(),
            gh_cli: false,
            no_ui: true,
        };

        // without credentials: the challenge is reported, nothing gets through
        static CHALLENGES: std::sync::Mutex<Vec<(String, bool)>> =
            std::sync::Mutex::new(Vec::new());
        fn record(request: corvene_platform::front_door::AuthRequired) {
            CHALLENGES
                .lock()
                .unwrap()
                .push((request.server, request.rejected));
        }
        corvene_platform::front_door::set_auth_required_handler(Some(record));
        corvene_platform::front_door::load(Some(Default::default()));
        corvene_git::set_extra_headers(Some(corvene_platform::front_door::git_headers));
        // sign-in: the browser flow's token exchange, then the account
        let sign_in = || -> Result<_, corvene_github::error::GitHubError> {
            let flow = corvene_github::auth::WebFlow::new("http://127.0.0.1:1/callback")?;
            let (token, _scopes) = flow.exchange_code(&endpoint, "client-id", None, "code")?;
            assert_eq!(token, "stub-token");
            let client = corvene_github::api::Client::new(endpoint.clone(), token);
            client.current_user(Vec::new())
        };
        assert!(sign_in().is_err(), "the front door let a request through");
        assert_eq!(
            CHALLENGES.lock().unwrap().last(),
            Some(&(server.clone(), false))
        );
        let refused = scratch.join("refused");
        let options = corvene_git::CloneOptions {
            askpass: Some(askpass()),
            ..Default::default()
        };
        let url = format!("{base}/octocat/hello.git");
        assert!(
            corvene_git::clone_with_options(git.clone(), &url, &refused, &options, None, |_| {})
                .is_err(),
            "git got through the front door"
        );

        // wrong credentials: refused, and reported as such
        corvene_platform::front_door::set(&server, Some(("door".into(), "wrong".into())));
        assert!(sign_in().is_err());
        assert_eq!(
            CHALLENGES.lock().unwrap().last(),
            Some(&(server.clone(), true))
        );

        // the right ones
        corvene_platform::front_door::set(&server, Some(("door".into(), "s3cret".into())));
        let account = sign_in().unwrap_or_else(|err| panic!("sign-in: {err}"));
        assert_eq!(account.login, "octocat");
        assert_eq!(account.emails, ["mona@example.com"]);
        // later API calls, with the token as signed in
        let client = corvene_github::api::Client::new(endpoint.clone(), "stub-token");
        assert_eq!(
            client.token_scopes().unwrap(),
            Some(vec!["repo".into(), "user".into(), "workflow".into()])
        );

        let work = scratch.join("work");
        let options = corvene_git::CloneOptions {
            askpass: Some(askpass()),
            ..Default::default()
        };
        corvene_git::clone_with_options(git.clone(), &url, &work, &options, None, |_| {})
            .unwrap_or_else(|err| panic!("clone: {err}"));
        // the remote URL carries no token: git asks the askpass for it
        corvene_git::remote_ops::fetch(
            git.clone(),
            &work,
            "origin",
            Some(&askpass()),
            &mut |_, _| {},
        )
        .unwrap_or_else(|err| panic!("fetch: {err}"));
        std::fs::write(work.join("pushed.txt"), "through the front door\n").unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        run(&work, &["add", "pushed.txt"]);
        run(
            &work,
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "pushed"],
        );
        corvene_git::remote_ops::push(
            git.clone(),
            &work,
            "origin",
            "main",
            None,
            &[],
            false,
            Some(&askpass()),
            &mut |_, _| {},
        )
        .unwrap_or_else(|err| panic!("push: {err}"));
        corvene_platform::front_door::load(None);
        corvene_git::set_extra_headers(None);
    }
}
