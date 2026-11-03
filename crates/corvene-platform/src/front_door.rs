//! Corvene (`530-front-door-credentials`, desktop#8783): a GitHub
//! Enterprise server behind a reverse proxy (a "front door") that wants
//! HTTP Basic credentials of its own before anything reaches GitHub.
//!
//! GHD 3.6.6 sends nothing but the token (`app/src/lib/http.ts`
//! `getAPIHeaders`: `Authorization: token …`), and the proxy's 401 is a
//! failed request; git asks for a username and password, and the token
//! goes where the proxy wants its own. The proxy's credentials and the
//! token cannot share the one `Authorization` header, so Corvene never
//! puts the proxy's there next to a token. For a server whose credentials
//! are saved ([`set`], by `host:port`), every request of Corvene's own
//! ([`before`], from `proxy::per_request`) and every git operation
//! ([`git_headers`], as `http.<url>.extraHeader`) carries:
//!
//! - `Proxy-Authorization: Basic …`, the separate header for credentials
//!   meant for an intermediary, which proxies set up for API clients read;
//! - the cookies the proxy sets once it has seen its credentials in
//!   `Authorization` (its session, the way a browser gets through). Those
//!   come from one request of Corvene's own to the server's root with only
//!   the proxy's credentials ([`preflight`]), made before the first request
//!   that needs them and again after the proxy refuses one.
//!
//! The token stays in `Authorization` (git sends its own `Basic` with the
//! token once GitHub asks). A front door that reads nothing but
//! `Authorization` cannot be combined with a token by any client.
//!
//! A 401 with a `Basic` challenge and no `X-GitHub-Request-Id` (GitHub
//! marks every response of its own with one) comes from the proxy: it is
//! reported ([`set_auth_required_handler`]) so the app can ask for the
//! credentials, and the session cookies are dropped.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use tracing::{debug, info};
use ureq::http::{HeaderMap, HeaderValue, Uri};

/// A front door answered 401 with its own challenge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthRequired {
    /// The server's `host:port`.
    pub server: String,
    /// Credentials were sent and refused.
    pub rejected: bool,
}

#[derive(Clone, Debug, Default)]
struct Door {
    user: String,
    password: String,
    /// The proxy's session cookies, `(name, value)`.
    cookies: Vec<(String, String)>,
    /// [`preflight`] ran since the last refusal.
    preflighted: bool,
}

/// `None` while the flag is off; else the saved servers by `host:port`.
static DOORS: RwLock<Option<HashMap<String, Door>>> = RwLock::new(None);

static HANDLER: RwLock<Option<fn(AuthRequired)>> = RwLock::new(None);

/// Turns front-door credentials on with these `(username, password)` pairs
/// per server `host:port`, or off (`None`).
pub fn load(credentials: Option<HashMap<String, (String, String)>>) {
    if let Ok(mut slot) = DOORS.write() {
        *slot = credentials.map(|map| {
            map.into_iter()
                .map(|(key, (user, password))| {
                    (
                        key.to_ascii_lowercase(),
                        Door {
                            user,
                            password,
                            ..Door::default()
                        },
                    )
                })
                .collect()
        });
    }
}

/// The credentials for the server `key` (`host:port`), or none. Ignored
/// while off.
pub fn set(key: &str, credentials: Option<(String, String)>) {
    if let Ok(mut slot) = DOORS.write()
        && let Some(map) = slot.as_mut()
    {
        let key = key.to_ascii_lowercase();
        match credentials {
            Some((user, password)) => {
                map.insert(
                    key,
                    Door {
                        user,
                        password,
                        ..Door::default()
                    },
                );
            }
            None => {
                map.remove(&key);
            }
        }
    }
}

fn enabled() -> bool {
    DOORS.read().is_ok_and(|slot| slot.is_some())
}

fn door(key: &str) -> Option<Door> {
    DOORS.read().ok()?.as_ref()?.get(key).cloned()
}

fn update(key: &str, f: impl FnOnce(&mut Door)) {
    if let Ok(mut slot) = DOORS.write()
        && let Some(door) = slot.as_mut().and_then(|map| map.get_mut(key))
    {
        f(door);
    }
}

/// Where front-door challenges go (the app asks for credentials).
pub fn set_auth_required_handler(handler: Option<fn(AuthRequired)>) {
    if let Ok(mut slot) = HANDLER.write() {
        *slot = handler;
    }
}

/// `host:port` of `uri`, the port the scheme's default when not given.
pub fn server_key(uri: &Uri) -> Option<String> {
    let host = uri.host()?.to_ascii_lowercase();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") {
            80
        } else {
            443
        });
    Some(crate::proxy::proxy_key(&host, port))
}

/// A 401 whose `Basic` challenge did not come from GitHub.
pub fn is_challenge(status: u16, headers: &HeaderMap) -> bool {
    status == 401
        && !headers.contains_key("x-github-request-id")
        && headers.get_all("www-authenticate").iter().any(|value| {
            value
                .to_str()
                .is_ok_and(|v| v.trim_start().to_ascii_lowercase().starts_with("basic"))
        })
}

fn basic(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        crate::proxy::base64_encode(format!("{user}:{password}").as_bytes())
    )
}

fn cookie_header(cookies: &[(String, String)]) -> Option<String> {
    (!cookies.is_empty()).then(|| {
        cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// The `(name, value)` of a `Set-Cookie` header; `None` for one that
/// deletes the cookie (`Max-Age=0`) or cannot be read.
fn parse_set_cookie(value: &str) -> Option<(String, Option<String>)> {
    let mut parts = value.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let deleted = parts.any(|attr| {
        attr.trim()
            .split_once('=')
            .is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("max-age") && v.trim() == "0")
    });
    Some((
        name.to_string(),
        (!deleted).then(|| value.trim().to_string()),
    ))
}

/// Keep the session cookies `headers` set.
fn take_cookies(door: &mut Door, headers: &HeaderMap) {
    for value in headers.get_all("set-cookie") {
        let Some((name, value)) = value.to_str().ok().and_then(parse_set_cookie) else {
            continue;
        };
        door.cookies.retain(|(n, _)| *n != name);
        if let Some(value) = value {
            door.cookies.push((name, value));
        }
    }
}

/// A request made by [`preflight`] (it carries its own credentials).
#[derive(Clone, Copy, Debug)]
struct Preflight;

/// `GET <scheme>://<server>/` with only the proxy's credentials, in
/// `Authorization`, without following redirects: the cookies the proxy sets
/// are its session for the requests that carry a token.
fn preflight(scheme: &str, key: &str) {
    let Some(door) = door(key) else {
        return;
    };
    update(key, |door| door.preflighted = true);
    let agent = crate::proxy::agent(
        ureq::Agent::config_builder()
            .middleware(crate::proxy::per_request)
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build(),
    );
    let url = format!("{scheme}://{key}/");
    let request = ureq::http::Request::get(&url)
        .header("Authorization", basic(&door.user, &door.password))
        .extension(Preflight)
        .body(());
    match request
        .map_err(ureq::Error::from)
        .and_then(|r| agent.run(r))
    {
        Ok(response) => debug!(server = key, status = %response.status(), "front door preflight"),
        Err(err) => debug!(server = key, %err, "front door preflight failed"),
    }
}

/// What [`after`] needs to know about a request [`before`] saw.
#[derive(Clone, Debug)]
pub struct Sent {
    key: String,
    credentials: bool,
}

/// From `proxy::per_request`, before a request goes out: a request to a
/// server with saved credentials gets `Proxy-Authorization` and the proxy's
/// session cookies (looked up first when there are none yet). `None` while
/// off.
pub fn before(request: &mut ureq::http::Request<ureq::SendBody>) -> Option<Sent> {
    if !enabled() {
        return None;
    }
    let key = server_key(request.uri())?;
    let preflight_request = request.extensions().get::<Preflight>().is_some();
    let Some(mut door) = door(&key) else {
        return Some(Sent {
            key,
            credentials: false,
        });
    };
    if !preflight_request && !door.preflighted && door.cookies.is_empty() {
        let scheme = request.uri().scheme_str().unwrap_or("https").to_string();
        preflight(&scheme, &key);
        door = self::door(&key).unwrap_or(door);
    }
    let headers = request.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&basic(&door.user, &door.password)) {
        headers.insert("proxy-authorization", value);
    }
    if !preflight_request && let Some(cookies) = cookie_header(&door.cookies) {
        let merged = match headers.get("cookie").and_then(|v| v.to_str().ok()) {
            Some(existing) => format!("{existing}; {cookies}"),
            None => cookies,
        };
        if let Ok(value) = HeaderValue::from_str(&merged) {
            headers.insert("cookie", value);
        }
    }
    Some(Sent {
        key,
        credentials: true,
    })
}

/// From `proxy::per_request`, with the response to a request [`before`]
/// saw: the proxy's cookies are kept, and its challenge drops them and is
/// reported.
pub fn after(sent: &Sent, status: u16, headers: &HeaderMap) {
    update(&sent.key, |door| take_cookies(door, headers));
    if !is_challenge(status, headers) {
        return;
    }
    update(&sent.key, |door| {
        door.cookies.clear();
        door.preflighted = false;
    });
    info!(
        server = sent.key,
        rejected = sent.credentials,
        "the server's front door wants credentials"
    );
    if let Some(handler) = HANDLER.read().ok().and_then(|slot| *slot) {
        handler(AuthRequired {
            server: sent.key.clone(),
            rejected: sent.credentials,
        });
    }
}

/// The headers git sends to `remote_url`'s server (`http.<url>.extraHeader`):
/// `Proxy-Authorization` and the proxy's session cookies, looked up first
/// when there are none yet. Empty for a server without saved credentials,
/// SSH remotes and while off.
pub fn git_headers(remote_url: &str) -> Vec<String> {
    if !enabled() {
        return Vec::new();
    }
    let Ok(uri) = remote_url.parse::<Uri>() else {
        return Vec::new();
    };
    let Some(scheme) = uri
        .scheme_str()
        .filter(|s| s.eq_ignore_ascii_case("https") || s.eq_ignore_ascii_case("http"))
    else {
        return Vec::new();
    };
    let Some(key) = server_key(&uri) else {
        return Vec::new();
    };
    let Some(mut door) = door(&key) else {
        return Vec::new();
    };
    if !door.preflighted && door.cookies.is_empty() {
        preflight(&scheme.to_ascii_lowercase(), &key);
        door = self::door(&key).unwrap_or(door);
    }
    let mut headers = vec![format!(
        "Proxy-Authorization: {}",
        basic(&door.user, &door.password)
    )];
    if let Some(cookies) = cookie_header(&door.cookies) {
        headers.push(format!("Cookie: {cookies}"));
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn only_a_basic_challenge_from_outside_github_counts() {
        let proxy = headers(&[("www-authenticate", "Basic realm=\"corp\"")]);
        assert!(is_challenge(401, &proxy));
        assert!(!is_challenge(403, &proxy));
        // GitHub's own 401 (git's smart HTTP asks for the token this way)
        let github = headers(&[
            ("www-authenticate", "Basic realm=\"GitHub\""),
            ("x-github-request-id", "C0FE:1234"),
        ]);
        assert!(!is_challenge(401, &github));
        let bearer = headers(&[("www-authenticate", "Bearer")]);
        assert!(!is_challenge(401, &bearer));
    }

    #[test]
    fn set_cookie_values_and_deletions() {
        assert_eq!(
            parse_set_cookie("session=abc123; Path=/; HttpOnly; Secure"),
            Some(("session".into(), Some("abc123".into())))
        );
        assert_eq!(
            parse_set_cookie("session=; Max-Age=0; Path=/"),
            Some(("session".into(), None))
        );
        assert_eq!(parse_set_cookie("garbage"), None);
        let mut door = Door::default();
        take_cookies(
            &mut door,
            &headers(&[("set-cookie", "a=1; Path=/"), ("set-cookie", "b=2")]),
        );
        take_cookies(&mut door, &headers(&[("set-cookie", "a=3")]));
        assert_eq!(cookie_header(&door.cookies).as_deref(), Some("b=2; a=3"));
        take_cookies(&mut door, &headers(&[("set-cookie", "b=x; Max-Age=0")]));
        assert_eq!(cookie_header(&door.cookies).as_deref(), Some("a=3"));
    }

    #[test]
    fn server_keys_carry_the_default_port() {
        let key = |url: &str| server_key(&url.parse().unwrap());
        assert_eq!(
            key("https://GHE.corp/api/v3/user").as_deref(),
            Some("ghe.corp:443")
        );
        assert_eq!(key("http://ghe.corp/x").as_deref(), Some("ghe.corp:80"));
        assert_eq!(
            key("https://ghe.corp:8443/").as_deref(),
            Some("ghe.corp:8443")
        );
    }

    #[test]
    fn basic_credentials_are_base64() {
        assert_eq!(
            basic("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }
}
