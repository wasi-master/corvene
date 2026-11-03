//! The proxy for Corvene's own HTTPS requests (the GitHub API, sign-in,
//! avatars, updates, packs, extensions) and the system proxy lookup git's
//! remote operations use (`corvene_git::proxy`). GHD makes its requests
//! through Chromium, which follows the system's proxy settings, PAC scripts
//! and WPAD; ureq on its own reads only the proxy environment variables,
//! which an app started from the Dock or Finder does not have
//! (desktop#13975).
//!
//! [`proxy_for`] takes, per request URL and in order: the environment
//! (`https_proxy` / `HTTPS_PROXY` for https, `http_proxy` for http, then
//! `ALL_PROXY` / `all_proxy`, with `NO_PROXY`: cURL's rules), git's global
//! `http.proxy` (handed over with [`set_git_http_proxy`]: only corvene-git
//! runs git), and the system proxy ([`system_proxies`]): on macOS the
//! network settings through CFNetwork, PAC scripts and WPAD included; on
//! Windows WinHTTP (the Internet Options settings, WPAD and PAC); on Linux
//! the XDG desktop portal's `ProxyResolver`. Loopback addresses never go
//! through a proxy (Chromium's rule). Deviation (`.docs/deviations.md`):
//! SOCKS proxies are skipped for these requests (ureq is built without
//! them), so a PAC answer of `SOCKS5 …; DIRECT` connects directly.
//!
//! Corvene (`528-proxy-credentials`, desktop#10026): a proxy that answers
//! 407 is reported to the app ([`set_auth_required_handler`]), which asks
//! for a username and password and hands them back with
//! [`set_credentials`]. [`agent`] builds ureq agents whose CONNECT step
//! sends them as `Proxy-Authorization: Basic` (ureq's own CONNECT can only
//! send credentials that are valid in a URL; Basic is the only scheme
//! either supports). GHD 3.6.6 has no proxy authentication at all: its
//! main process (`app/src/main-process/main.ts`) handles no `login` event,
//! so Chromium cancels a 407.

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};
use ureq::http::Uri;
use ureq::unversioned::transport::{
    ConnectProxyConnector, ConnectionDetails, Connector, Either, RustlsConnector, TcpConnector,
    Transport, TransportAdapter,
};

static GIT_HTTP_PROXY: RwLock<Option<String>> = RwLock::new(None);

/// git's global `http.proxy`, read by the core at launch.
pub fn set_git_http_proxy(proxy: Option<String>) {
    if let Ok(mut slot) = GIT_HTTP_PROXY.write() {
        *slot = proxy.filter(|p| !p.trim().is_empty());
    }
}

fn git_http_proxy() -> Option<String> {
    GIT_HTTP_PROXY.read().ok().and_then(|slot| slot.clone())
}

// ---------------------------------------------------------------------------
// proxies
// ---------------------------------------------------------------------------

/// The protocol a proxy speaks (PAC's `PROXY`, `HTTPS`, `SOCKS4`, `SOCKS5`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProxyScheme {
    Http,
    Https,
    Socks4,
    Socks5,
}

impl ProxyScheme {
    fn parse(scheme: &str) -> Option<Self> {
        match scheme.to_ascii_lowercase().as_str() {
            "http" | "proxy" => Some(Self::Http),
            "https" => Some(Self::Https),
            "socks4" | "socks4a" => Some(Self::Socks4),
            "socks" | "socks5" | "socks5h" => Some(Self::Socks5),
            _ => None,
        }
    }

    fn url_scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Socks4 => "socks4",
            Self::Socks5 => "socks5",
        }
    }

    fn pac_keyword(self) -> &'static str {
        match self {
            Self::Http => "PROXY",
            Self::Https => "HTTPS",
            Self::Socks4 => "SOCKS4",
            Self::Socks5 => "SOCKS5",
        }
    }
}

/// One proxy server.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProxyServer {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
}

impl ProxyServer {
    /// `host:port`, what credentials are kept by.
    pub fn key(&self) -> String {
        proxy_key(&self.host, self.port)
    }

    /// `http://host:port`.
    pub fn url(&self) -> String {
        format!("{}://{}", self.scheme.url_scheme(), self.key())
    }

    /// cURL's (and git's) proxy URL for `value` as `http.proxy` or a
    /// `*_proxy` variable hold it: the scheme defaults to `http://` and the
    /// port to 1080, cURL's default for every proxy type. `None` for a value
    /// that names no host.
    pub fn from_proxy_value(value: &str) -> Option<(Self, bool)> {
        let value = value.trim();
        let (scheme, rest) = match value.split_once("://") {
            Some((scheme, rest)) => (ProxyScheme::parse(scheme)?, rest),
            None => (ProxyScheme::Http, value),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let (userinfo, host_port) = match authority.rsplit_once('@') {
            Some((userinfo, host_port)) => (Some(userinfo), host_port),
            None => (None, authority),
        };
        let (host, port) = split_host_port(host_port)?;
        Some((
            Self {
                scheme,
                host,
                // cURL: 1080 for every proxy type but HTTPS (443)
                port: port.unwrap_or(if scheme == ProxyScheme::Https {
                    443
                } else {
                    1080
                }),
            },
            userinfo.is_some_and(|u| !u.is_empty()),
        ))
    }
}

/// `host:port` with the host in lower case (IPv6 in brackets).
pub fn proxy_key(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// `host[:port]` → (host, port); IPv6 literals keep their brackets off.
fn split_host_port(value: &str) -> Option<(String, Option<u16>)> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
        return (!host.is_empty()).then(|| (host.to_ascii_lowercase(), port));
    }
    let (host, port) = match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host, port.parse().ok()),
        _ => (value, None),
    };
    (!host.is_empty()).then(|| (host.to_ascii_lowercase(), port))
}

/// Where the proxy for a request came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxySource {
    /// `https_proxy` and friends.
    Environment,
    /// git's `http.proxy`.
    GitConfig,
    /// The system's proxy settings.
    System,
    /// A proxy auto-configuration script (a PAC URL or WPAD).
    AutoConfig,
}

impl ProxySource {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Environment => "the environment",
            Self::GitConfig => "Git's http.proxy",
            Self::System => "the system settings",
            Self::AutoConfig => "the proxy auto-configuration script",
        }
    }
}

/// The proxy a request goes through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyChoice {
    pub server: ProxyServer,
    pub source: ProxySource,
    /// The URL ureq is given: the environment's or `http.proxy`'s value
    /// (credentials included), else [`ProxyServer::url`].
    pub url: String,
    /// The value carries its own credentials (`http://user:pass@host`).
    pub url_credentials: bool,
}

/// The proxy `url` goes through, or `None` to connect directly.
pub fn proxy_for(url: &str) -> Option<ProxyChoice> {
    let target = TargetUrl::parse(url)?;
    if target.is_loopback() {
        return None;
    }
    if let Some(choice) = environment_proxy(&target, &|key| std::env::var(key).ok()) {
        return choice;
    }
    if let Some(value) = git_http_proxy() {
        return match ProxyServer::from_proxy_value(&value) {
            Some((server, url_credentials)) => Some(ProxyChoice {
                server,
                source: ProxySource::GitConfig,
                url: value.clone(),
                url_credentials,
            }),
            None => {
                warn!("ignoring git's http.proxy");
                None
            }
        };
    }
    let lookup = system_proxies(url);
    let source = if lookup.auto_config {
        ProxySource::AutoConfig
    } else {
        ProxySource::System
    };
    // PAC lists are tried in order; ureq speaks only HTTP(S) proxies
    lookup
        .servers
        .into_iter()
        .find(|server| matches!(server.scheme, ProxyScheme::Http | ProxyScheme::Https))
        .map(|server| ProxyChoice {
            url: server.url(),
            server,
            source,
            url_credentials: false,
        })
}

/// A request URL's parts that proxy selection needs.
struct TargetUrl {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl TargetUrl {
    fn parse(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host, port) = split_host_port(host_port)?;
        Some(Self {
            scheme: scheme.to_ascii_lowercase(),
            host,
            port,
        })
    }

    fn is_loopback(&self) -> bool {
        self.host == "localhost"
            || self.host.ends_with(".localhost")
            || self
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }
}

/// cURL's environment rules for `target`: `Some(None)` when `NO_PROXY`
/// exempts it (the environment decided: direct), `None` when the
/// environment names no proxy for it.
fn environment_proxy(
    target: &TargetUrl,
    var: &dyn Fn(&str) -> Option<String>,
) -> Option<Option<ProxyChoice>> {
    let non_empty = |key: &str| var(key).filter(|v| !v.trim().is_empty());
    // cURL reads `http_proxy` in lower case only
    let scheme_vars: &[&str] = match target.scheme.as_str() {
        "https" => &["https_proxy", "HTTPS_PROXY"],
        "http" => &["http_proxy"],
        _ => &[],
    };
    let value = scheme_vars
        .iter()
        .chain(&["all_proxy", "ALL_PROXY"])
        .find_map(|key| non_empty(key))?;
    let no_proxy = non_empty("no_proxy").or_else(|| non_empty("NO_PROXY"));
    if no_proxy.is_some_and(|list| no_proxy_matches(&list, &target.host)) {
        return Some(None);
    }
    match ProxyServer::from_proxy_value(&value) {
        Some((server, url_credentials)) => Some(Some(ProxyChoice {
            server,
            source: ProxySource::Environment,
            url: value,
            url_credentials,
        })),
        None => {
            warn!("ignoring the proxy environment variable");
            None
        }
    }
}

/// cURL's `NO_PROXY`: `*`, or host names that match themselves and their
/// subdomains (a leading dot is optional), separated by commas or spaces;
/// a `:port` suffix is ignored.
fn no_proxy_matches(list: &str, host: &str) -> bool {
    list.split([',', ' '])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            if entry == "*" {
                return true;
            }
            let entry = split_host_port(entry).map_or_else(|| entry.to_string(), |(h, _)| h);
            let entry = entry.trim_start_matches('.');
            host == entry || host.ends_with(&format!(".{entry}"))
        })
}

// ---------------------------------------------------------------------------
// the system proxy
// ---------------------------------------------------------------------------

/// What the system answers for a URL: its proxies in order (empty:
/// direct), and whether a PAC script or WPAD chose them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemLookup {
    pub servers: Vec<ProxyServer>,
    pub auto_config: bool,
}

/// Lookups are kept per origin this long: a PAC script may be fetched and
/// run for each one.
const SYSTEM_CACHE_TTL: Duration = Duration::from_secs(300);

static SYSTEM_CACHE: Mutex<Vec<(String, Instant, SystemLookup)>> = Mutex::new(Vec::new());

/// The system's proxies for `url`. Blocks while a PAC script is fetched and
/// run (up to a few seconds), so never call it on the main thread.
pub fn system_proxies(url: &str) -> SystemLookup {
    let Some(target) = TargetUrl::parse(url) else {
        return SystemLookup::default();
    };
    if target.is_loopback() {
        return SystemLookup::default();
    }
    let origin = format!(
        "{}://{}",
        target.scheme,
        target
            .port
            .map_or_else(|| target.host.clone(), |p| proxy_key(&target.host, p))
    );
    let now = Instant::now();
    if let Ok(cache) = SYSTEM_CACHE.lock()
        && let Some((_, _, lookup)) = cache
            .iter()
            .find(|(o, at, _)| *o == origin && now.duration_since(*at) < SYSTEM_CACHE_TTL)
    {
        return lookup.clone();
    }
    let lookup = imp::system_proxies(url).unwrap_or_else(|err| {
        warn!(%err, "system proxy lookup failed");
        SystemLookup::default()
    });
    debug!(origin, ?lookup, "system proxy");
    if let Ok(mut cache) = SYSTEM_CACHE.lock() {
        cache.retain(|(o, at, _)| *o != origin && now.duration_since(*at) < SYSTEM_CACHE_TTL);
        cache.push((origin, now, lookup.clone()));
    }
    lookup
}

/// [`system_proxies`] as a PAC string (`"PROXY host:port; DIRECT"`), what
/// Electron's `session.resolveProxy` gives GHD's `resolveGitProxy`
/// (`corvene_git::proxy::set_system_proxy_resolver`).
pub fn system_pac_string(url: &str) -> Result<String, String> {
    Ok(pac_string(&system_proxies(url).servers))
}

fn pac_string(servers: &[ProxyServer]) -> String {
    servers
        .iter()
        .map(|s| format!("{} {}", s.scheme.pac_keyword(), s.key()))
        .chain(std::iter::once("DIRECT".to_string()))
        .collect::<Vec<_>>()
        .join("; ")
}

// ---------------------------------------------------------------------------
// credentials
// ---------------------------------------------------------------------------

/// `None` while `528-proxy-credentials` is off; else the credentials per
/// proxy `host:port`.
static CREDENTIALS: RwLock<Option<HashMap<String, (String, String)>>> = RwLock::new(None);

static AUTH_HANDLER: RwLock<Option<fn(AuthRequired)>> = RwLock::new(None);

/// A proxy answered 407 Proxy Authentication Required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthRequired {
    /// The proxy's `host:port`.
    pub proxy: String,
    /// Credentials were sent and refused.
    pub rejected: bool,
}

/// Turns proxy credentials on with these (username, password) pairs per
/// proxy `host:port`, or off (`None`): 407s are reported only while on.
pub fn load_credentials(credentials: Option<HashMap<String, (String, String)>>) {
    if let Ok(mut slot) = CREDENTIALS.write() {
        *slot = credentials.map(|map| {
            map.into_iter()
                .map(|(key, pair)| (key.to_ascii_lowercase(), pair))
                .collect()
        });
    }
}

/// The username and password for the proxy `key` (`host:port`), or none.
/// Ignored while credentials are off.
pub fn set_credentials(key: &str, credentials: Option<(String, String)>) {
    if let Ok(mut slot) = CREDENTIALS.write()
        && let Some(map) = slot.as_mut()
    {
        match credentials {
            Some(pair) => map.insert(key.to_ascii_lowercase(), pair),
            None => map.remove(&key.to_ascii_lowercase()),
        };
    }
}

fn credentials(key: &str) -> Option<(String, String)> {
    CREDENTIALS
        .read()
        .ok()?
        .as_ref()?
        .get(&key.to_ascii_lowercase())
        .cloned()
}

fn credentials_enabled() -> bool {
    CREDENTIALS.read().is_ok_and(|slot| slot.is_some())
}

/// Where 407s go (the app shows a prompt); `None` stops reporting.
pub fn set_auth_required_handler(handler: Option<fn(AuthRequired)>) {
    if let Ok(mut slot) = AUTH_HANDLER.write() {
        *slot = handler;
    }
}

fn report_auth_required(proxy: String, rejected: bool) {
    if !credentials_enabled() {
        return;
    }
    info!(proxy, rejected, "proxy wants credentials");
    if let Some(handler) = AUTH_HANDLER.read().ok().and_then(|slot| *slot) {
        handler(AuthRequired { proxy, rejected });
    }
}

/// The request failed because the proxy wants (other) credentials.
pub fn is_auth_required(err: &ureq::Error) -> bool {
    match err {
        ureq::Error::ConnectProxyFailed(reason) => reason.contains("407"),
        ureq::Error::StatusCode(407) => true,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// ureq
// ---------------------------------------------------------------------------

/// An agent for `config`, which must carry [`per_request`] as middleware:
/// the CONNECT step sends the saved proxy credentials.
pub fn agent(config: ureq::config::Config) -> ureq::Agent {
    let connector =
        ().chain(CredentialConnect)
            .chain(ConnectProxyConnector::default())
            .chain(TcpConnector::default())
            .chain(RustlsConnector::default());
    ureq::Agent::with_parts(
        config,
        connector,
        ureq::unversioned::resolver::DefaultResolver::default(),
    )
}

/// ureq middleware: the request goes through [`proxy_for`] its URL, and a
/// 407 is reported ([`set_auth_required_handler`]).
pub fn per_request(
    request: ureq::http::Request<ureq::SendBody>,
    next: ureq::middleware::MiddlewareNext,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    use ureq::RequestExt as _;

    let choice = proxy_for(&request.uri().to_string());
    let key = choice.as_ref().map(|choice| choice.server.key());
    let saved = key.as_deref().and_then(credentials);
    // an HTTPS proxy's saved credentials go in its URL (`CredentialConnect`
    // speaks plain HTTP only), as far as a URL can hold them
    let mut sent = false;
    let proxy = choice.as_ref().and_then(|choice| {
        let embedded = match (&saved, choice.server.scheme, choice.url_credentials) {
            (Some((user, password)), ProxyScheme::Https, false) => {
                ureq::Proxy::builder(ureq::ProxyProtocol::Https)
                    .host(&choice.server.host)
                    .port(choice.server.port)
                    .username(user)
                    .password(password)
                    .build()
                    .ok()
            }
            _ => None,
        };
        sent = embedded.is_some();
        embedded.or_else(|| {
            ureq::Proxy::new(&ureq_proxy_url(choice))
                .inspect_err(|err| warn!(%err, url = %choice.server.url(), "ignoring the proxy"))
                .ok()
        })
    });
    let mut request = match request.middleware_config() {
        Some(config) => config.proxy(proxy.clone()).build(),
        // not inside an agent's middleware chain: cannot happen
        None => return Err(ureq::Error::BadUri("no agent".into())),
    };
    // `530-front-door-credentials`: a server behind a proxy of its own
    let front_door = crate::front_door::before(&mut request);
    let result = next.handle(request);
    if let (Some(sent), Ok(response)) = (&front_door, &result) {
        crate::front_door::after(sent, response.status().as_u16(), response.headers());
    }
    if let (Some(choice), Some(key), Some(_)) = (&choice, key, &proxy) {
        let refused = match &result {
            Ok(response) => response.status().as_u16() == 407,
            Err(err) => is_auth_required(err),
        };
        // `CredentialConnect` reports the refusals of the credentials it
        // sends (an HTTP proxy's)
        let reported_by_connect = choice.server.scheme == ProxyScheme::Http && saved.is_some();
        if refused && !choice.url_credentials && !reported_by_connect {
            report_auth_required(key, sent);
        }
    }
    result
}

/// The URL ureq is given: the configured value when it carries its own
/// credentials, else the proxy with an explicit port, so ureq connects
/// where git (cURL) does and credentials are kept under the same
/// `host:port` (a value without a port means cURL's default, where ureq
/// would take the scheme's).
fn ureq_proxy_url(choice: &ProxyChoice) -> String {
    if !choice.url_credentials {
        return choice.server.url();
    }
    if choice.url.contains("://") {
        choice.url.clone()
    } else {
        format!("http://{}", choice.url.trim())
    }
}

/// ureq's `ConnectProxyConnector` with the saved credentials: it opens the
/// tunnel itself when an HTTP proxy without credentials in its URL has
/// some saved, writing `Proxy-Authorization` from the raw username and
/// password (ureq's own takes them from the proxy URL, which cannot hold a
/// space, `#`, `/`, `?` and the like). Anything else is left to ureq.
#[derive(Debug)]
struct CredentialConnect;

impl<In: Transport> Connector<In> for CredentialConnect {
    type Out = Either<In, Box<dyn Transport>>;

    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        if let Some(transport) = chained {
            return Ok(Some(Either::A(transport)));
        }
        let Some(proxy) = details.config.proxy() else {
            return Ok(None);
        };
        if proxy.protocol() != ureq::ProxyProtocol::Http
            || proxy.username().is_some()
            || proxy.is_no_proxy(details.uri)
        {
            return Ok(None);
        }
        let key = proxy_key(proxy.host(), proxy.port());
        let Some((username, password)) = credentials(&key) else {
            return Ok(None);
        };
        let proxy_addrs = details
            .resolver
            .resolve(proxy.uri(), details.config, details.timeout)?;
        let proxy_details = ConnectionDetails {
            uri: proxy.uri(),
            addrs: proxy_addrs,
            config: details.config,
            request_level: details.request_level,
            resolver: details.resolver,
            now: details.now,
            timeout: details.timeout,
            current_time: details.current_time.clone(),
            run_connector: details.run_connector.clone(),
        };
        let tcp = match Connector::<()>::connect(&TcpConnector::default(), &proxy_details, None)? {
            Some(Either::B(tcp)) => tcp,
            _ => return Err(ureq::Error::ConnectionFailed),
        };
        let target = details.uri;
        let host = target.host().ok_or(ureq::Error::HostNotFound)?;
        let port = target.port_u16().unwrap_or_else(|| default_port(target));
        let mut w = TransportAdapter::new(tcp.boxed());
        write!(w, "CONNECT {host}:{port} HTTP/1.1\r\n")?;
        write!(w, "Host: {host}:{port}\r\n")?;
        if let ureq::config::AutoHeaderValue::Provided(agent) = details.config.user_agent() {
            write!(w, "User-Agent: {agent}\r\n")?;
        }
        write!(w, "Proxy-Connection: Keep-Alive\r\n")?;
        write!(
            w,
            "Proxy-Authorization: Basic {}\r\n\r\n",
            base64_encode(format!("{username}:{password}").as_bytes())
        )?;
        w.flush()?;
        let mut transport = w.into_inner();
        let status = loop {
            let progressed = transport.await_input(details.timeout)?;
            let buffers = transport.buffers();
            let input = buffers.input();
            if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                let status = connect_status(&input[..end]);
                buffers.input_consume(end + 4);
                break status;
            }
            if !progressed {
                return Err(ureq::Error::ConnectProxyFailed(
                    "proxy server did not respond".into(),
                ));
            }
        };
        match status {
            Some(200) => Ok(Some(Either::B(transport))),
            Some(code) => {
                if code == 407 {
                    report_auth_required(key, true);
                }
                Err(ureq::Error::ConnectProxyFailed(format!(
                    "proxy server responded {code}/{code}"
                )))
            }
            None => Err(ureq::Error::ConnectProxyFailed(
                "proxy server sent no status".into(),
            )),
        }
    }
}

fn default_port(uri: &Uri) -> u16 {
    if uri.scheme_str() == Some("http") {
        80
    } else {
        443
    }
}

/// The status code of a CONNECT response head (`HTTP/1.1 200 OK`).
fn connect_status(head: &[u8]) -> Option<u16> {
    let line = head.split(|b| *b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    parts.next().filter(|v| v.starts_with("HTTP/"))?;
    parts.next()?.parse().ok()
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(TABLE[(n >> shift) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// platforms
// ---------------------------------------------------------------------------

/// macOS: `CFNetworkCopyProxiesForURL` over `CFNetworkCopySystemProxySettings`
/// (the network settings, their bypass list included), running a PAC
/// script it points at with `CFNetworkExecuteProxyAutoConfigurationURL` /
/// `…Script` on this thread's run loop (Chromium's
/// `ProxyResolverMac`). With "Auto proxy discovery" on and nothing else
/// set, `http://wpad/wpad.dat` is tried (WPAD over DNS).
/// `CORVENE_PROXY_PAC=<url>` runs that PAC script instead (development).
#[cfg(target_os = "macos")]
mod imp {
    #![allow(unexpected_cfgs)] // `objc` macros probe a `cargo-clippy` feature
    #![allow(non_upper_case_globals)]

    use std::ffi::{CString, c_void};
    use std::time::{Duration, Instant};

    use objc::runtime::{BOOL, NO, Object};
    use objc::{class, msg_send, sel, sel_impl};

    use super::{ProxyScheme, ProxyServer, SystemLookup};

    type CFTypeRef = *const c_void;
    type PacCallback = extern "C" fn(client: *mut c_void, proxies: CFTypeRef, error: CFTypeRef);

    #[repr(C)]
    struct CFStreamClientContext {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
    }

    #[link(name = "CFNetwork", kind = "framework")]
    unsafe extern "C" {
        fn CFNetworkCopySystemProxySettings() -> CFTypeRef;
        fn CFNetworkCopyProxiesForURL(url: CFTypeRef, settings: CFTypeRef) -> CFTypeRef;
        fn CFNetworkExecuteProxyAutoConfigurationURL(
            pac_url: CFTypeRef,
            target: CFTypeRef,
            callback: PacCallback,
            context: *mut CFStreamClientContext,
        ) -> CFTypeRef;
        fn CFNetworkExecuteProxyAutoConfigurationScript(
            script: CFTypeRef,
            target: CFTypeRef,
            callback: PacCallback,
            context: *mut CFStreamClientContext,
        ) -> CFTypeRef;
        static kCFProxyTypeKey: CFTypeRef;
        static kCFProxyHostNameKey: CFTypeRef;
        static kCFProxyPortNumberKey: CFTypeRef;
        static kCFProxyAutoConfigurationURLKey: CFTypeRef;
        static kCFProxyAutoConfigurationJavaScriptKey: CFTypeRef;
        static kCFProxyTypeNone: CFTypeRef;
        static kCFProxyTypeHTTP: CFTypeRef;
        static kCFProxyTypeHTTPS: CFTypeRef;
        static kCFProxyTypeSOCKS: CFTypeRef;
        static kCFProxyTypeAutoConfigurationURL: CFTypeRef;
        static kCFProxyTypeAutoConfigurationJavaScript: CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFRunLoopGetCurrent() -> CFTypeRef;
        fn CFRunLoopAddSource(run_loop: CFTypeRef, source: CFTypeRef, mode: CFTypeRef);
        fn CFRunLoopRemoveSource(run_loop: CFTypeRef, source: CFTypeRef, mode: CFTypeRef);
        fn CFRunLoopRunInMode(mode: CFTypeRef, seconds: f64, return_after_source: u8) -> i32;
        fn CFRunLoopStop(run_loop: CFTypeRef);
    }

    /// How long one PAC script may take (fetch + run); a lookup that runs
    /// out goes direct and is cached like any other.
    const PAC_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn system_proxies(url: &str) -> Result<SystemLookup, String> {
        // SAFETY: every CF object created or copied here is released before
        // returning; Foundation objects are autoreleased into the pool,
        // which is drained after the last use
        unsafe {
            let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
            let result = match std::env::var("CORVENE_PROXY_PAC") {
                Ok(pac) if !pac.is_empty() => lookup_pac(url, &pac),
                _ => lookup(url),
            };
            let _: () = msg_send![pool, drain];
            result
        }
    }

    /// `CORVENE_PROXY_PAC=<url>` (development): this PAC script instead of
    /// the system settings.
    unsafe fn lookup_pac(url: &str, pac: &str) -> Result<SystemLookup, String> {
        // SAFETY: see `system_proxies`
        unsafe {
            let target: *mut Object =
                msg_send![class!(NSURL), URLWithString: ns_string(url).ok_or("bad URL")?];
            let pac_url: *mut Object =
                msg_send![class!(NSURL), URLWithString: ns_string(pac).ok_or("bad URL")?];
            if target.is_null() || pac_url.is_null() {
                return Err(format!("not a URL: {url} / {pac}"));
            }
            let mut lookup = SystemLookup {
                servers: Vec::new(),
                auto_config: true,
            };
            if let Some(list) = run_pac(|cb, ctx| {
                CFNetworkExecuteProxyAutoConfigurationURL(pac_url.cast(), target.cast(), cb, ctx)
            }) {
                walk(list as *mut Object, target, &mut lookup, false);
                CFRelease(list);
            }
            Ok(lookup)
        }
    }

    unsafe fn lookup(url: &str) -> Result<SystemLookup, String> {
        // SAFETY: see `system_proxies`
        unsafe {
            let target: *mut Object =
                msg_send![class!(NSURL), URLWithString: ns_string(url).ok_or("bad URL")?];
            if target.is_null() {
                return Err(format!("not a URL: {url}"));
            }
            let settings = CFNetworkCopySystemProxySettings();
            if settings.is_null() {
                return Ok(SystemLookup::default());
            }
            let list = CFNetworkCopyProxiesForURL(target.cast(), settings);
            let mut lookup = SystemLookup::default();
            if !list.is_null() {
                walk(list as *mut Object, target, &mut lookup, true);
                CFRelease(list);
            }
            let wpad = lookup.servers.is_empty()
                && !lookup.auto_config
                && number(settings as *mut Object, "ProxyAutoDiscoveryEnable").unwrap_or(0) != 0;
            if wpad {
                let wpad_url: *mut Object = msg_send![
                    class!(NSURL),
                    URLWithString: ns_string("http://wpad/wpad.dat").ok_or("bad URL")?
                ];
                if let Some(list) = run_pac(|cb, ctx| {
                    CFNetworkExecuteProxyAutoConfigurationURL(
                        wpad_url.cast(),
                        target.cast(),
                        cb,
                        ctx,
                    )
                }) {
                    lookup.auto_config = true;
                    walk(list as *mut Object, target, &mut lookup, false);
                    CFRelease(list);
                }
            }
            CFRelease(settings);
            Ok(lookup)
        }
    }

    /// Adds the proxies of a CFNetwork proxy list up to its first direct
    /// entry; `nested` runs the PAC scripts it names.
    unsafe fn walk(list: *mut Object, target: *mut Object, out: &mut SystemLookup, nested: bool) {
        // SAFETY: `list` is a live `NSArray` of dictionaries
        unsafe {
            let is_array: BOOL = msg_send![list, isKindOfClass: class!(NSArray)];
            if is_array == NO {
                return;
            }
            let count: usize = msg_send![list, count];
            for i in 0..count {
                let entry: *mut Object = msg_send![list, objectAtIndex: i];
                let Some(kind) = value(entry, kCFProxyTypeKey) else {
                    continue;
                };
                if equal(kind, kCFProxyTypeNone) {
                    return;
                }
                let scheme = if equal(kind, kCFProxyTypeHTTP) || equal(kind, kCFProxyTypeHTTPS) {
                    // "HTTPS" is the proxy for https:// URLs, an HTTP proxy
                    // all the same (Chromium's `GetProxyServerScheme`)
                    Some(ProxyScheme::Http)
                } else if equal(kind, kCFProxyTypeSOCKS) {
                    Some(ProxyScheme::Socks5)
                } else {
                    None
                };
                if let Some(scheme) = scheme {
                    let host = value(entry, kCFProxyHostNameKey).and_then(|h| string(h));
                    let port = value(entry, kCFProxyPortNumberKey)
                        .and_then(|p| number_value(p))
                        .and_then(|p| u16::try_from(p).ok());
                    if let (Some(host), Some(port)) = (host, port)
                        && !host.is_empty()
                    {
                        out.servers.push(ProxyServer {
                            scheme,
                            host: host.to_ascii_lowercase(),
                            port,
                        });
                    }
                    continue;
                }
                if !nested {
                    continue;
                }
                let result = if equal(kind, kCFProxyTypeAutoConfigurationURL) {
                    value(entry, kCFProxyAutoConfigurationURLKey).and_then(|pac_url| {
                        run_pac(|cb, ctx| {
                            CFNetworkExecuteProxyAutoConfigurationURL(
                                pac_url.cast(),
                                target.cast(),
                                cb,
                                ctx,
                            )
                        })
                    })
                } else if equal(kind, kCFProxyTypeAutoConfigurationJavaScript) {
                    value(entry, kCFProxyAutoConfigurationJavaScriptKey).and_then(|script| {
                        run_pac(|cb, ctx| {
                            CFNetworkExecuteProxyAutoConfigurationScript(
                                script.cast(),
                                target.cast(),
                                cb,
                                ctx,
                            )
                        })
                    })
                } else {
                    continue;
                };
                out.auto_config = true;
                // a script that fails to load or run means direct, as in
                // Chromium
                if let Some(list) = result {
                    walk(list as *mut Object, target, out, false);
                    CFRelease(list);
                }
                return;
            }
        }
    }

    struct PacState {
        done: bool,
        result: CFTypeRef,
    }

    extern "C" fn pac_done(client: *mut c_void, proxies: CFTypeRef, error: CFTypeRef) {
        // SAFETY: `client` is the `PacState` `run_pac` passed, alive until
        // its run loop returns; the list is retained before the callback
        // returns (CFNetwork releases it afterwards)
        unsafe {
            let state = &mut *(client as *mut PacState);
            if error.is_null() && !proxies.is_null() {
                state.result = CFRetain(proxies);
            } else if !error.is_null() {
                tracing::warn!("proxy auto-configuration script failed");
            }
            state.done = true;
            CFRunLoopStop(CFRunLoopGetCurrent());
        }
    }

    /// Runs one PAC execution (`start` gets the callback and context) on a
    /// private mode of this thread's run loop; the retained proxy list, or
    /// `None` when it failed or timed out.
    unsafe fn run_pac(
        start: impl FnOnce(PacCallback, *mut CFStreamClientContext) -> CFTypeRef,
    ) -> Option<CFTypeRef> {
        // SAFETY: the state is only reached through `state` (the callback
        // gets the same pointer) and freed after the source is removed and
        // released, so no callback can run afterwards
        unsafe {
            let state = Box::into_raw(Box::new(PacState {
                done: false,
                result: std::ptr::null(),
            }));
            let mut context = CFStreamClientContext {
                version: 0,
                info: state.cast(),
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
            };
            let source = start(pac_done, &mut context);
            if source.is_null() {
                drop(Box::from_raw(state));
                return None;
            }
            let Some(mode) = ns_string("com.wasimaster.corvene.proxy") else {
                CFRelease(source);
                drop(Box::from_raw(state));
                return None;
            };
            let run_loop = CFRunLoopGetCurrent();
            CFRunLoopAddSource(run_loop, source, mode.cast());
            let deadline = Instant::now() + PAC_TIMEOUT;
            loop {
                if (*state).done {
                    break;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    tracing::warn!("proxy auto-configuration script timed out");
                    break;
                }
                CFRunLoopRunInMode(mode.cast(), left.as_secs_f64(), 1);
            }
            CFRunLoopRemoveSource(run_loop, source, mode.cast());
            CFRelease(source);
            let state = Box::from_raw(state);
            (!state.result.is_null()).then_some(state.result)
        }
    }

    unsafe fn ns_string(value: &str) -> Option<*mut Object> {
        let value = CString::new(value).ok()?;
        // SAFETY: an autoreleased `NSString` copy of a C string
        unsafe {
            let ns: *mut Object = msg_send![class!(NSString), stringWithUTF8String: value.as_ptr()];
            (!ns.is_null()).then_some(ns)
        }
    }

    /// `[dict objectForKey:key]` for a CF key constant.
    unsafe fn value(dict: *mut Object, key: CFTypeRef) -> Option<*mut Object> {
        // SAFETY: `dict` is a live dictionary and `key` a CFString constant
        unsafe {
            let is_dict: BOOL = msg_send![dict, isKindOfClass: class!(NSDictionary)];
            if is_dict == NO {
                return None;
            }
            let value: *mut Object = msg_send![dict, objectForKey: key as *mut Object];
            (!value.is_null()).then_some(value)
        }
    }

    unsafe fn equal(a: *mut Object, b: CFTypeRef) -> bool {
        // SAFETY: `a` is a live object, `b` a CFString constant
        unsafe {
            let same: BOOL = msg_send![a, isEqual: b as *mut Object];
            same != NO
        }
    }

    unsafe fn number(dict: *mut Object, key: &str) -> Option<i64> {
        // SAFETY: as in `value`
        unsafe {
            let key = ns_string(key)?;
            let value: *mut Object = msg_send![dict, objectForKey: key];
            if value.is_null() {
                return None;
            }
            number_value(value)
        }
    }

    unsafe fn number_value(value: *mut Object) -> Option<i64> {
        // SAFETY: `NSNumber` answers `longLongValue`, and so does an
        // `NSString` of digits
        unsafe {
            let responds: BOOL = msg_send![value, respondsToSelector: sel!(longLongValue)];
            (responds != NO).then(|| msg_send![value, longLongValue])
        }
    }

    unsafe fn string(value: *mut Object) -> Option<String> {
        // SAFETY: only strings are converted; the UTF-8 buffer lives as long
        // as the autoreleased string
        unsafe {
            let is_string: BOOL = msg_send![value, isKindOfClass: class!(NSString)];
            if is_string == NO {
                return None;
            }
            let utf8: *const std::ffi::c_char = msg_send![value, UTF8String];
            (!utf8.is_null()).then(|| {
                std::ffi::CStr::from_ptr(utf8)
                    .to_string_lossy()
                    .into_owned()
            })
        }
    }
}

/// Windows: WinHTTP. With "Automatically detect settings" or a setup
/// script in Internet Options, `WinHttpGetProxyForUrl` runs WPAD / the PAC
/// script; else the manual proxy (`http=host:port;https=…` or one
/// `host:port` for all) and its bypass list (`<local>` included) apply.
#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY,
        WINHTTP_AUTO_DETECT_TYPE_DHCP, WINHTTP_AUTO_DETECT_TYPE_DNS_A,
        WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL, WINHTTP_AUTOPROXY_OPTIONS,
        WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WINHTTP_PROXY_INFO, WinHttpCloseHandle,
        WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl, WinHttpOpen,
    };
    use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, w};

    use super::{ProxyScheme, ProxyServer, SystemLookup, TargetUrl, split_host_port};

    /// Takes a WinHTTP-allocated string (and frees it).
    unsafe fn take(value: PWSTR) -> Option<String> {
        if value.is_null() {
            return None;
        }
        // SAFETY: WinHTTP hands out NUL-terminated strings from GlobalAlloc
        unsafe {
            let text = value.to_string().ok();
            let _ = GlobalFree(Some(HGLOBAL(value.0.cast())));
            text.filter(|t| !t.trim().is_empty())
        }
    }

    pub fn system_proxies(url: &str) -> Result<SystemLookup, String> {
        let target = TargetUrl::parse(url).ok_or("bad URL")?;
        let mut ie = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
        // SAFETY: fills `ie`; its strings are taken (and freed) right after
        unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut ie) }.map_err(|e| e.to_string())?;
        let auto_detect = ie.fAutoDetect.as_bool();
        // SAFETY: each string once
        let (config_url, manual, bypass) = unsafe {
            (
                take(ie.lpszAutoConfigUrl),
                take(ie.lpszProxy),
                take(ie.lpszProxyBypass),
            )
        };
        if auto_detect || config_url.is_some() {
            match auto_proxy(url, auto_detect, config_url.as_deref()) {
                Ok(Some(list)) => {
                    return Ok(SystemLookup {
                        servers: parse_list(&list, &target.scheme),
                        auto_config: true,
                    });
                }
                Ok(None) => {
                    return Ok(SystemLookup {
                        servers: Vec::new(),
                        auto_config: true,
                    });
                }
                // no WPAD server or a broken script: the manual settings
                Err(err) => tracing::debug!(%err, "WinHTTP auto proxy"),
            }
        }
        let Some(manual) = manual else {
            return Ok(SystemLookup::default());
        };
        if bypass.is_some_and(|list| bypassed(&list, &target.host)) {
            return Ok(SystemLookup::default());
        }
        Ok(SystemLookup {
            servers: parse_list(&manual, &target.scheme),
            auto_config: false,
        })
    }

    /// The `host:port` list WPAD / the PAC script gives, `None` for direct.
    fn auto_proxy(
        url: &str,
        auto_detect: bool,
        config_url: Option<&str>,
    ) -> Result<Option<String>, String> {
        // SAFETY: the session handle is closed below; the URL strings live
        // across the call
        unsafe {
            let session = WinHttpOpen(
                w!("Corvene"),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            );
            if session.is_null() {
                return Err("WinHttpOpen failed".into());
            }
            let config_url = config_url.map(HSTRING::from);
            let mut options = WINHTTP_AUTOPROXY_OPTIONS {
                fAutoLogonIfChallenged: BOOL::from(true),
                ..Default::default()
            };
            if auto_detect {
                options.dwFlags |= WINHTTP_AUTOPROXY_AUTO_DETECT;
                options.dwAutoDetectFlags =
                    WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A;
            }
            if let Some(config_url) = &config_url {
                options.dwFlags |= WINHTTP_AUTOPROXY_CONFIG_URL;
                options.lpszAutoConfigUrl = PCWSTR(config_url.as_ptr());
            }
            let mut info = WINHTTP_PROXY_INFO::default();
            let result =
                WinHttpGetProxyForUrl(session, &HSTRING::from(url), &mut options, &mut info);
            let _ = WinHttpCloseHandle(session);
            result.map_err(|e| e.to_string())?;
            let list = take(info.lpszProxy);
            let _ = take(info.lpszProxyBypass);
            Ok((info.dwAccessType == WINHTTP_ACCESS_TYPE_NAMED_PROXY)
                .then_some(list)
                .flatten())
        }
    }

    /// `host:port; host2:port` or `http=host:port;https=host:port`: the
    /// entries for `scheme` (or for every scheme) in order.
    fn parse_list(list: &str, scheme: &str) -> Vec<ProxyServer> {
        list.split([';', ' '])
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .filter_map(|entry| {
                let (for_scheme, value) = match entry.split_once('=') {
                    Some((s, v)) => (Some(s.to_ascii_lowercase()), v),
                    None => (None, entry),
                };
                let proxy_scheme = match for_scheme.as_deref() {
                    None => ProxyScheme::Http,
                    Some(s) if s == scheme => ProxyScheme::Http,
                    Some("socks") => ProxyScheme::Socks4,
                    _ => return None,
                };
                let value = value
                    .split_once("://")
                    .map_or(value, |(_, rest)| rest)
                    .trim_end_matches('/');
                let (host, port) = split_host_port(value)?;
                Some(ProxyServer {
                    scheme: proxy_scheme,
                    host,
                    port: port.unwrap_or(80),
                })
            })
            .collect()
    }

    /// The manual settings' bypass list: `<local>` (names without a dot),
    /// and wildcard patterns (`*.corp`, `10.*`).
    fn bypassed(list: &str, host: &str) -> bool {
        list.split([';', ' '])
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .any(|entry| {
                if entry == "<local>" {
                    return !host.contains('.');
                }
                let entry = entry.split_once("://").map_or(&*entry, |(_, r)| r);
                wildcard(entry, host)
            })
    }

    fn wildcard(pattern: &str, text: &str) -> bool {
        match pattern.split_once('*') {
            None => pattern == text,
            Some((head, tail)) => {
                let Some(rest) = text.strip_prefix(head) else {
                    return false;
                };
                (0..=rest.len())
                    .filter(|i| rest.is_char_boundary(*i))
                    .any(|i| wildcard(tail, &rest[i..]))
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn manual_lists() {
            let servers = parse_list("http=a:1;https=b:2", "https");
            assert_eq!(servers.len(), 1);
            assert_eq!(servers[0].key(), "b:2");
            assert_eq!(
                parse_list("proxy.corp:3128", "https")[0].key(),
                "proxy.corp:3128"
            );
            assert!(bypassed("<local>;*.corp", "intranet"));
            assert!(bypassed("<local>;*.corp", "git.corp"));
            assert!(!bypassed("<local>;*.corp", "github.com"));
        }
    }
}

/// Linux and the BSDs: the XDG desktop portal's `ProxyResolver` (GNOME's
/// and KDE's settings, PAC scripts included, through the portal's
/// GProxyResolver); no portal, no system proxy.
#[cfg(not(any(target_os = "macos", windows, target_os = "android")))]
mod imp {
    use std::sync::OnceLock;
    use std::time::Duration;

    use super::{ProxyScheme, ProxyServer, SystemLookup, split_host_port};

    pub fn system_proxies(url: &str) -> Result<SystemLookup, String> {
        static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
        let Some(bus) = BUS
            .get_or_init(|| {
                zbus::blocking::connection::Builder::session()
                    .ok()?
                    .method_timeout(Duration::from_secs(10))
                    .build()
                    .ok()
            })
            .as_ref()
        else {
            return Ok(SystemLookup::default());
        };
        let reply = bus
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.ProxyResolver"),
                "Lookup",
                &(url,),
            )
            .map_err(|e| e.to_string())?;
        let proxies: Vec<String> = reply.body().deserialize().map_err(|e| e.to_string())?;
        Ok(SystemLookup {
            servers: parse_uris(&proxies),
            auto_config: false,
        })
    }

    /// GProxyResolver's answers (`http://host:port`, `socks5://…`) up to
    /// the first `direct://`.
    fn parse_uris(proxies: &[String]) -> Vec<ProxyServer> {
        proxies
            .iter()
            .take_while(|uri| !uri.starts_with("direct://"))
            .filter_map(|uri| {
                let (scheme, rest) = uri.split_once("://")?;
                let scheme = match scheme {
                    "http" => ProxyScheme::Http,
                    "https" => ProxyScheme::Https,
                    "socks4" | "socks4a" => ProxyScheme::Socks4,
                    "socks" | "socks5" => ProxyScheme::Socks5,
                    _ => return None,
                };
                let authority = rest.split('/').next().unwrap_or(rest);
                let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
                let (host, port) = split_host_port(authority)?;
                Some(ProxyServer {
                    scheme,
                    host,
                    port: port.unwrap_or(match scheme {
                        ProxyScheme::Https => 443,
                        ProxyScheme::Http => 80,
                        _ => 1080,
                    }),
                })
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn portal_answers() {
            let list = super::parse_uris(&[
                "http://proxy.corp:3128".into(),
                "direct://".into(),
                "http://never:1".into(),
            ]);
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].key(), "proxy.corp:3128");
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use super::SystemLookup;

    pub fn system_proxies(_url: &str) -> Result<SystemLookup, String> {
        Ok(SystemLookup::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_values() {
        let (server, creds) = ProxyServer::from_proxy_value("proxy.corp:3128").unwrap();
        assert_eq!(
            (server.url(), creds),
            ("http://proxy.corp:3128".into(), false)
        );
        let (server, creds) = ProxyServer::from_proxy_value("http://u:p@Proxy.Corp").unwrap();
        assert_eq!((server.key(), creds), ("proxy.corp:1080".into(), true));
        let (server, _) = ProxyServer::from_proxy_value("socks5://[::1]:9050").unwrap();
        assert_eq!(
            (server.scheme, server.key()),
            (ProxyScheme::Socks5, "[::1]:9050".into())
        );
        assert!(ProxyServer::from_proxy_value("").is_none());
    }

    #[test]
    fn environment_rules() {
        let target = TargetUrl::parse("https://api.github.com/user").unwrap();
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            }
        };
        let choice = environment_proxy(&target, &env(&[("HTTPS_PROXY", "p:1")]));
        assert_eq!(choice.flatten().unwrap().server.key(), "p:1");
        // `HTTP_PROXY` is not cURL's
        assert!(environment_proxy(&target, &env(&[("HTTP_PROXY", "p:1")])).is_none());
        let exempt = env(&[("all_proxy", "p:1"), ("NO_PROXY", "localhost, .github.com")]);
        assert_eq!(environment_proxy(&target, &exempt), Some(None));
        assert!(no_proxy_matches("github.com:443", "api.github.com"));
        assert!(!no_proxy_matches("hub.com", "github.com"));
    }

    #[test]
    fn loopback_goes_direct() {
        assert!(
            TargetUrl::parse("http://127.0.0.1:8080/x")
                .unwrap()
                .is_loopback()
        );
        assert!(TargetUrl::parse("http://[::1]:8080").unwrap().is_loopback());
        assert!(
            !TargetUrl::parse("https://github.com")
                .unwrap()
                .is_loopback()
        );
        assert_eq!(
            system_proxies("http://localhost:1/"),
            SystemLookup::default()
        );
    }

    #[test]
    fn pac_strings() {
        let servers = vec![
            ProxyServer {
                scheme: ProxyScheme::Http,
                host: "a".into(),
                port: 1,
            },
            ProxyServer {
                scheme: ProxyScheme::Socks5,
                host: "b".into(),
                port: 2,
            },
        ];
        assert_eq!(pac_string(&servers), "PROXY a:1; SOCKS5 b:2; DIRECT");
        assert_eq!(pac_string(&[]), "DIRECT");
    }

    #[test]
    fn connect_heads_and_base64() {
        assert_eq!(connect_status(b"HTTP/1.1 407 Proxy Auth"), Some(407));
        assert_eq!(
            connect_status(b"HTTP/1.0 200 Connection established\r\nX: y"),
            Some(200)
        );
        assert_eq!(connect_status(b"garbage"), None);
        assert_eq!(
            base64_encode(b"proxyuser:s3cret pass"),
            "cHJveHl1c2VyOnMzY3JldCBwYXNz"
        );
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
    }

    #[test]
    fn system_proxy_lookup_does_not_crash() {
        // whatever the machine's settings are
        let _ = system_proxies("https://github.com");
    }
}
