//! The proxy git's remote operations use: GHD `envForProxy`
//! (`lib/git/environment.ts`), `resolveGitProxy` (`lib/resolve-git-proxy.ts`)
//! and `parsePACString` (`lib/parse-pac-string.ts`).
//!
//! GHD asks Electron (Chromium's system proxy resolution: the OS proxy
//! settings, PAC scripts, WPAD) for the PAC string of a remote's URL, turns
//! the first proxy it names into `http_proxy` / `https_proxy` for git, and
//! leaves the environment alone when the user already set one. Corvene's
//! lookup is `corvene_platform::proxy::system_pac_string`, plugged in with
//! [`set_system_proxy_resolver`] at launch. Clone, the remote operations of
//! `remote_ops` (fetch, pull, push, tag pushes and deletes, `remote
//! set-head`, `remote prune`), `delete_remote_branch`, and (by GHD's
//! `getFallbackUrlForProxyResolve`, [`env_for_fallback`]) checkout, revert
//! and submodule updates apply it. The environment git sees includes the
//! login shell's (`hook_env`), so a proxy set there counts as set (GHD
//! looks at its own `process.env` only).
//!
//! Corvene (`528-proxy-credentials`, desktop#10026): with a username saved
//! for the proxy git will use ([`set_proxy_logins`], [`effective_proxy`]),
//! the operation gets that proxy with the username in its URL as
//! `http.<remote URL>.proxy` (through `GIT_CONFIG_COUNT`, which beats
//! `http.proxy` and the environment), so git asks `GIT_ASKPASS` for the
//! password (`Password for 'http://user@host:port': `), which the askpass
//! answers from the keychain (`CORVENE_ASKPASS_PROXIES` names the proxies).
//! A 407 is `RemoteFailure::ProxyAuthenticationRequired`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use tracing::{info, warn};

use crate::detect::GitBinary;
use crate::process::GitCommand;

/// The PAC string (`"PROXY host:port; DIRECT"`) of the system proxy for a
/// URL: Electron's `session.resolveProxy`, which GHD's `resolveGitProxy`
/// calls.
pub type SystemProxyResolver = fn(&str) -> Result<String, String>;

static SYSTEM_PROXY_RESOLVER: RwLock<Option<SystemProxyResolver>> = RwLock::new(None);

/// Sets (or clears) the system proxy lookup [`env_for_remote_operation`]
/// uses.
pub fn set_system_proxy_resolver(resolver: Option<SystemProxyResolver>) {
    if let Ok(mut slot) = SYSTEM_PROXY_RESOLVER.write() {
        *slot = resolver;
    }
}

fn system_proxy_resolver() -> Option<SystemProxyResolver> {
    SYSTEM_PROXY_RESOLVER.read().ok().and_then(|slot| *slot)
}

/// GHD `parsePACString`: the cURL proxy URLs of a PAC string as Chromium
/// writes it (`ProxyList::ToPacString`), in order, up to the first `DIRECT`;
/// `None` when none is left. Protocols cURL does not speak (`QUIC`) and specs
/// without a host are skipped; the port is kept as given (cURL's default
/// proxy port is 1080, not the protocol's).
pub fn parse_pac_string(pac_string: &str) -> Option<Vec<String>> {
    // the happy path, for nearly everyone
    if pac_string == "DIRECT" {
        return None;
    }
    let mut urls = Vec::new();
    for spec in pac_string.trim().split(';').map(str::trim) {
        // cURL has no fallback to a direct connection: stop at the first
        if spec
            .get(..6)
            .is_some_and(|start| start.eq_ignore_ascii_case("direct"))
        {
            break;
        }
        // `spec.split(/\s+/, 2)`: the type and the first word after it
        let mut words = spec.split_whitespace();
        let (Some(protocol), Some(endpoint)) = (words.next(), words.next()) else {
            continue;
        };
        match url_from_protocol_and_endpoint(protocol, endpoint) {
            Some(url) => urls.push(url),
            None => warn!(spec, "skipping proxy spec"),
        }
    }
    (!urls.is_empty()).then_some(urls)
}

/// GHD `urlFromProtocolAndEndpoint`. `HTTP` is an alias for `PROXY`,
/// `SOCKS` for `SOCKS4`.
fn url_from_protocol_and_endpoint(protocol: &str, endpoint: &str) -> Option<String> {
    let scheme = match protocol.to_lowercase().as_str() {
        "proxy" | "http" => "http",
        "https" => "https",
        "socks" | "socks4" => "socks4",
        "socks5" => "socks5",
        _ => return None,
    };
    Some(format!("{scheme}://{endpoint}"))
}

/// GHD `resolveGitProxy`: the first proxy of the PAC string `resolve_proxy`
/// gives for `url` (a failing lookup counts as `DIRECT`). Windows skips
/// `https://` proxies, which git's `schannel` backend cannot use.
pub fn resolve_git_proxy(
    url: &str,
    resolve_proxy: impl Fn(&str) -> Result<String, String>,
) -> Option<String> {
    let pac_string = resolve_proxy(url).unwrap_or_else(|err| {
        warn!(url, %err, "failed resolving proxy");
        "DIRECT".to_string()
    });
    parse_pac_string(&pac_string)?.into_iter().find(|proxy| {
        let unsupported = cfg!(windows) && proxy.starts_with("https://");
        if unsupported {
            warn!("ignoring https proxy, not supported in cURL/schannel");
        }
        !unsupported
    })
}

/// GHD `envForProxy(remoteUrl, env, resolve)`: `http_proxy` or
/// `https_proxy` (by the remote URL's scheme, case-insensitive) set to what
/// `resolve` finds for the URL. `None` for any other scheme (only cURL
/// speaks to proxies; ssh and `git://` do not), when `ALL_PROXY` /
/// `all_proxy` or the scheme's own variable (for https also `HTTPS_PROXY`)
/// is already in `env`, and when `resolve` fails or finds nothing.
pub fn env_for_proxy(
    remote_url: &str,
    env: &HashMap<String, String>,
    resolve: &dyn Fn(&str) -> Result<Option<String>, String>,
) -> Option<HashMap<String, String>> {
    // `/^(https?):\/\//i`
    let scheme_end = remote_url.find("://")?;
    let protocol = remote_url[..scheme_end].to_ascii_lowercase();
    if protocol != "http" && protocol != "https" {
        return None;
    }
    // whoever set ALL_PROXY knows what they want: cURL reads both cases
    if env.contains_key("ALL_PROXY") || env.contains_key("all_proxy") {
        info!("proxy url not resolved, ALL_PROXY already set");
        return None;
    }
    // cURL reads `http_proxy` in lower case only
    let env_key = format!("{protocol}_proxy");
    if env.contains_key(&env_key) || (protocol == "https" && env.contains_key("HTTPS_PROXY")) {
        info!("proxy url not resolved, {env_key} already set");
        return None;
    }
    let proxy_url = resolve(remote_url).unwrap_or_else(|err| {
        warn!(%err, "failed resolving git proxy");
        None
    })?;
    Some(HashMap::from([(env_key, proxy_url)]))
}

/// GHD `envForRemoteOperation(remoteUrl)`'s proxy half: [`env_for_proxy`]
/// with the process environment and the system proxy, for the git command
/// of a remote operation on `remote_url`. Empty while no system proxy lookup
/// is set ([`set_system_proxy_resolver`]).
pub fn env_for_remote_operation(remote_url: &str) -> Vec<(String, String)> {
    env_for_remote_operation_in(remote_url, &git_environment())
}

/// [`env_for_remote_operation`] over the environment `env`.
fn env_for_remote_operation_in(
    remote_url: &str,
    env: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let Some(system) = system_proxy_resolver() else {
        return Vec::new();
    };
    env_for_proxy(remote_url, env, &|url| Ok(resolve_git_proxy(url, system)))
        .map(|vars| vars.into_iter().collect())
        .unwrap_or_default()
}

/// The environment git commands start from: Corvene's own plus the login
/// shell's (`hook_env`, which `process` puts on every command).
fn git_environment() -> HashMap<String, String> {
    let mut env = HashMap::new();
    let shell = crate::hook_env::hook_env().unwrap_or_default();
    let own = std::env::vars_os().map(|(key, value)| {
        (
            key.to_string_lossy().into_owned(),
            value.to_string_lossy().into_owned(),
        )
    });
    for (key, value) in own.chain(shell) {
        // Windows: Node's `process.env` (and cURL) ignore the case of names
        if cfg!(windows) {
            env.insert(key.to_lowercase(), value.clone());
            env.insert(key.to_uppercase(), value.clone());
        }
        env.insert(key, value);
    }
    env
}

/// [`env_for_remote_operation`] for the URL `remote_url` gives, which is
/// only asked for while a system proxy lookup is set or proxy credentials
/// are on (it may read the repository's configuration), plus the saved
/// proxy credentials ([`credential_env`]). `workdir` is the repository's
/// (`None` for a clone).
pub fn env_for_remote_operation_with(
    git: Arc<GitBinary>,
    workdir: Option<&Path>,
    remote_url: impl FnOnce() -> Option<String>,
) -> Vec<(String, String)> {
    proxy_env(git, workdir, remote_url, true)
}

/// The proxy variables for `remote_url`'s operation, the saved proxy
/// credentials only with `credentials` (the command has `GIT_ASKPASS` to
/// answer the password question).
fn proxy_env(
    git: Arc<GitBinary>,
    workdir: Option<&Path>,
    remote_url: impl FnOnce() -> Option<String>,
    credentials: bool,
) -> Vec<(String, String)> {
    let logins = proxy_logins().filter(|logins| credentials && !logins.is_empty());
    let extra_headers = extra_headers();
    if system_proxy_resolver().is_none() && logins.is_none() && extra_headers.is_none() {
        return Vec::new();
    }
    let Some(url) = remote_url() else {
        return Vec::new();
    };
    let base = git_environment();
    let mut env = env_for_remote_operation_in(&url, &base);
    if let Some(logins) = logins
        && let Some(proxy) = effective_proxy_in(git, workdir, &url, &base, &env)
    {
        let count = base.get("GIT_CONFIG_COUNT");
        env.extend(credential_env_at(&proxy, &url, &logins, count));
    }
    if let Some(extra_headers) = extra_headers {
        let headers = extra_headers(&url);
        let count = env
            .iter()
            .rev()
            .find(|(key, _)| key == "GIT_CONFIG_COUNT")
            .map(|(_, value)| value)
            .or_else(|| base.get("GIT_CONFIG_COUNT"));
        env.extend(extra_header_env(&url, &headers, count));
    }
    env
}

// ---------------------------------------------------------------------------
// extra headers (`530-front-door-credentials`)
// ---------------------------------------------------------------------------

/// The headers git sends to a remote URL's server: Corvene
/// (`530-front-door-credentials`), the credentials of a proxy in front of
/// a GitHub Enterprise server (`corvene_platform::front_door::git_headers`).
pub type ExtraHeaders = fn(&str) -> Vec<String>;

static EXTRA_HEADERS: RwLock<Option<ExtraHeaders>> = RwLock::new(None);

/// Sets (or clears) where the remote operations' extra headers come from.
pub fn set_extra_headers(provider: Option<ExtraHeaders>) {
    if let Ok(mut slot) = EXTRA_HEADERS.write() {
        *slot = provider;
    }
}

fn extra_headers() -> Option<ExtraHeaders> {
    EXTRA_HEADERS.read().ok().and_then(|slot| *slot)
}

/// `headers` as `http.<remote URL>.extraHeader` values through
/// `GIT_CONFIG_COUNT` (after the `count` entries already there): only
/// the remote's server gets them, and they stay out of the command line.
fn extra_header_env(
    remote_url: &str,
    headers: &[String],
    count: Option<&String>,
) -> Vec<(String, String)> {
    let lower = remote_url.to_ascii_lowercase();
    if headers.is_empty() || !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Vec::new();
    }
    let first = count
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let match_url = config_match_url(remote_url);
    let mut env = Vec::with_capacity(2 * headers.len() + 1);
    for (i, header) in headers.iter().enumerate() {
        env.push((
            format!("GIT_CONFIG_KEY_{}", first + i),
            format!("http.{match_url}.extraHeader"),
        ));
        env.push((format!("GIT_CONFIG_VALUE_{}", first + i), header.clone()));
    }
    env.push((
        "GIT_CONFIG_COUNT".to_string(),
        (first + headers.len()).to_string(),
    ));
    env
}

/// The URL as git matches `http.<url>.*`: no credentials, query or
/// fragment.
fn config_match_url(remote_url: &str) -> String {
    let (scheme, rest) = remote_url
        .split_once("://")
        .unwrap_or(("https", remote_url));
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let rest = match rest.split_once('/') {
        Some((authority, path)) => format!(
            "{}/{path}",
            authority.rsplit_once('@').map_or(authority, |(_, h)| h)
        ),
        None => rest.rsplit_once('@').map_or(rest, |(_, h)| h).to_string(),
    };
    format!("{scheme}://{rest}")
}

/// GHD `getFallbackUrlForProxyResolve` for the operations that are not
/// about one remote but may still download (checkout and revert run LFS
/// filters, submodule updates clone): the proxy environment for `origin`'s
/// URL (an SSH remote counts as `https://<its host>`, GHD takes the GitHub
/// repository's page), else `https://github.com`.
///
/// The saved proxy credentials come along only with `askpass` (git asks it
/// for the password); checkout and revert run without one.
pub fn env_for_fallback(
    git: Arc<GitBinary>,
    workdir: &Path,
    askpass: bool,
) -> Vec<(String, String)> {
    proxy_env(
        git.clone(),
        Some(workdir),
        || {
            let origin = crate::remote_ops::config_value(git, workdir, "remote.origin.url");
            Some(
                origin
                    .as_deref()
                    .and_then(https_url_for)
                    .unwrap_or_else(|| "https://github.com".to_string()),
            )
        },
        askpass,
    )
}

/// A remote URL as the https URL its proxy is looked up for: http(s) as
/// is, `ssh://[user@]host[:port]/…` and `user@host:path` as
/// `https://host`; `None` for paths and other schemes.
fn https_url_for(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(url.to_string());
    }
    let host = if let Some(rest) = lower.strip_prefix("ssh://") {
        let authority = rest.split('/').next()?;
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        host.split(':').next()?.to_string()
    } else if !lower.contains("://") && !lower.starts_with('/') {
        // scp-like `user@host:path`
        let (before, _) = lower.split_once(':')?;
        let host = before.rsplit_once('@').map_or(before, |(_, h)| h);
        if host.is_empty() || host.contains('/') || host.contains('\\') {
            return None;
        }
        host.to_string()
    } else {
        return None;
    };
    (!host.is_empty()).then(|| format!("https://{host}"))
}

// ---------------------------------------------------------------------------
// credentials (`528-proxy-credentials`)
// ---------------------------------------------------------------------------

/// `None` while proxy credentials are off; else the saved username per
/// proxy `host:port`.
static PROXY_LOGINS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// Turns proxy credentials on (`Some`, the usernames saved per proxy
/// `host:port`) or off.
pub fn set_proxy_logins(logins: Option<HashMap<String, String>>) {
    if let Ok(mut slot) = PROXY_LOGINS.write() {
        *slot = logins.map(|logins| {
            logins
                .into_iter()
                .map(|(key, user)| (key.to_ascii_lowercase(), user))
                .collect()
        });
    }
}

fn proxy_logins() -> Option<HashMap<String, String>> {
    PROXY_LOGINS.read().ok().and_then(|slot| slot.clone())
}

/// Where git's proxy for a URL comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitProxySource {
    /// `http.proxy` / `http.<url>.proxy`.
    Config,
    /// `https_proxy` and friends (the system proxy Corvene sets included).
    Environment,
}

/// The proxy git connects to for a remote URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitProxy {
    /// `http`, `https`, `socks5`, …
    pub scheme: String,
    /// `host:port` (cURL's default port when none is given: 443 for an
    /// HTTPS proxy, else 1080).
    pub key: String,
    /// The value names a user (`http://user[:pass]@host`): git asks for or
    /// sends those credentials itself.
    pub has_user: bool,
    pub source: GitProxySource,
}

/// The proxy git uses for `remote_url` in `workdir` (`None`: anywhere,
/// for a clone), with the proxy environment Corvene would add.
pub fn effective_proxy(
    git: Arc<GitBinary>,
    workdir: Option<&Path>,
    remote_url: &str,
) -> Option<GitProxy> {
    let base = git_environment();
    let added = env_for_remote_operation_in(remote_url, &base);
    effective_proxy_in(git, workdir, remote_url, &base, &added)
}

/// git's (cURL's) choice for `remote_url`: `http.<url>.proxy` /
/// `http.proxy` (`git config --get-urlmatch`; empty means none), else the
/// environment (`added` over `base`), with `no_proxy`. `None` for SSH and
/// local remotes.
fn effective_proxy_in(
    git: Arc<GitBinary>,
    workdir: Option<&Path>,
    remote_url: &str,
    base: &HashMap<String, String>,
    added: &[(String, String)],
) -> Option<GitProxy> {
    let scheme_end = remote_url.find("://")?;
    let protocol = remote_url[..scheme_end].to_ascii_lowercase();
    if protocol != "http" && protocol != "https" {
        return None;
    }
    let mut cmd = GitCommand::new(git)
        .args(["config", "--get-urlmatch", "http.proxy", remote_url])
        .allow_exit_code(1);
    if let Some(workdir) = workdir {
        cmd = cmd.current_dir(workdir);
    }
    let configured = cmd
        .run()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| out.stdout_string().ok())
        .map(|value| value.trim().to_string());
    if let Some(value) = configured {
        return (!value.is_empty())
            .then(|| parse_proxy_value(&value, GitProxySource::Config))
            .flatten();
    }
    let var = |key: &str| {
        added
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .or_else(|| base.get(key))
            .filter(|v| !v.trim().is_empty())
            .cloned()
    };
    let value = if protocol == "https" {
        var("https_proxy").or_else(|| var("HTTPS_PROXY"))
    } else {
        var("http_proxy")
    }
    .or_else(|| var("all_proxy"))
    .or_else(|| var("ALL_PROXY"))?;
    let host = url_host(remote_url)?;
    let no_proxy = var("no_proxy").or_else(|| var("NO_PROXY"));
    if no_proxy.is_some_and(|list| no_proxy_matches(&list, &host)) {
        return None;
    }
    parse_proxy_value(&value, GitProxySource::Environment)
}

/// A proxy value as cURL reads it: `[scheme://][user[:pass]@]host[:port]`.
fn parse_proxy_value(value: &str, source: GitProxySource) -> Option<GitProxy> {
    let value = value.trim();
    let (scheme, rest) = value.split_once("://").unwrap_or(("http", value));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let (user, host_port) = match authority.rsplit_once('@') {
        Some((user, host_port)) => (!user.is_empty(), host_port),
        None => (false, authority),
    };
    let (host, port) = split_host_port(host_port)?;
    let scheme = scheme.to_ascii_lowercase();
    // cURL: 1080 for every proxy type but HTTPS (443)
    let port = port.unwrap_or(if scheme == "https" { 443 } else { 1080 });
    let key = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Some(GitProxy {
        scheme,
        key,
        has_user: user,
        source,
    })
}

fn split_host_port(value: &str) -> Option<(String, Option<u16>)> {
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

fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    split_host_port(host_port).map(|(host, _)| host)
}

/// cURL's `no_proxy` (as `corvene_platform::proxy` reads it).
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

/// The variables that make git use `proxy` with its saved username for
/// `remote_url` (see the module doc); nothing when the proxy's value names
/// a user already or none is saved.
pub fn credential_env(
    proxy: &GitProxy,
    remote_url: &str,
    logins: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let base = git_environment();
    credential_env_at(proxy, remote_url, logins, base.get("GIT_CONFIG_COUNT"))
}

/// [`credential_env`] after the `GIT_CONFIG_COUNT` entries git already
/// gets (`count`).
fn credential_env_at(
    proxy: &GitProxy,
    remote_url: &str,
    logins: &HashMap<String, String>,
    count: Option<&String>,
) -> Vec<(String, String)> {
    if proxy.has_user || !matches!(proxy.scheme.as_str(), "http" | "https") {
        return Vec::new();
    }
    let Some(user) = logins.get(&proxy.key) else {
        return Vec::new();
    };
    let match_url = config_match_url(remote_url);
    let index = count
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    vec![
        (
            format!("GIT_CONFIG_KEY_{index}"),
            format!("http.{match_url}.proxy"),
        ),
        (
            format!("GIT_CONFIG_VALUE_{index}"),
            format!("{}://{}@{}", proxy.scheme, percent_encode(user), proxy.key),
        ),
        ("GIT_CONFIG_COUNT".to_string(), (index + 1).to_string()),
        ("CORVENE_ASKPASS_PROXIES".to_string(), proxy.key.clone()),
    ]
}

/// RFC 3986 userinfo: everything but unreserved characters escaped.
fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// git's 407 messages (cURL's wording changed over the years).
pub fn is_proxy_auth_failure(stderr: &str) -> bool {
    [
        "CONNECT tunnel failed, response 407",
        "Received HTTP code 407 from proxy",
        "The requested URL returned error: 407",
        "407 Proxy Authentication Required",
    ]
    .iter()
    .any(|needle| stderr.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_headers_go_to_the_remote_after_other_config_entries() {
        let headers = vec![
            "Proxy-Authorization: Basic dTpw".to_string(),
            "Cookie: s=1".to_string(),
        ];
        let env = extra_header_env(
            "https://me:token@ghe.corp/org/repo.git?x#y",
            &headers,
            Some(&"2".to_string()),
        );
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            pairs,
            [
                (
                    "GIT_CONFIG_KEY_2",
                    "http.https://ghe.corp/org/repo.git.extraHeader"
                ),
                ("GIT_CONFIG_VALUE_2", "Proxy-Authorization: Basic dTpw"),
                (
                    "GIT_CONFIG_KEY_3",
                    "http.https://ghe.corp/org/repo.git.extraHeader"
                ),
                ("GIT_CONFIG_VALUE_3", "Cookie: s=1"),
                ("GIT_CONFIG_COUNT", "4"),
            ]
        );
        assert!(extra_header_env("git@ghe.corp:org/repo.git", &headers, None).is_empty());
        assert!(extra_header_env("https://ghe.corp/r.git", &[], None).is_empty());
    }

    #[test]
    fn resolves_the_first_usable_proxy() {
        let pac = |_: &str| Ok("SOCKS5 a:1; PROXY b:2".to_string());
        assert_eq!(
            resolve_git_proxy("https://x", pac).as_deref(),
            Some("socks5://a:1")
        );
        let failing = |_: &str| Err("no".to_string());
        assert_eq!(resolve_git_proxy("https://x", failing), None);
    }

    #[test]
    fn no_resolver_no_variables() {
        set_system_proxy_resolver(None);
        set_proxy_logins(None);
        assert!(env_for_remote_operation("https://github.com/a/b.git").is_empty());
        // the remote's URL is not even read
        let read = std::cell::Cell::new(false);
        let git = Arc::new(crate::find_git().unwrap());
        let env = env_for_remote_operation_with(git, None, || {
            read.set(true);
            Some("https://github.com/a/b.git".to_string())
        });
        assert!(env.is_empty() && !read.get());
    }

    #[test]
    fn proxy_values_and_credentials() {
        let proxy = parse_proxy_value("Proxy.Corp:3128", GitProxySource::Config).unwrap();
        assert_eq!(
            (proxy.scheme.as_str(), proxy.key.as_str()),
            ("http", "proxy.corp:3128")
        );
        assert!(
            parse_proxy_value("http://u@p", GitProxySource::Config)
                .unwrap()
                .has_user
        );
        assert_eq!(
            parse_proxy_value("p", GitProxySource::Config).unwrap().key,
            "p:1080"
        );
        let logins = HashMap::from([("proxy.corp:3128".to_string(), "dom\\me".to_string())]);
        let env: HashMap<_, _> =
            credential_env(&proxy, "https://tok@github.com/o/r.git?x=1", &logins)
                .into_iter()
                .collect();
        let index = env["GIT_CONFIG_COUNT"].parse::<usize>().unwrap() - 1;
        assert_eq!(
            env[&format!("GIT_CONFIG_KEY_{index}")],
            "http.https://github.com/o/r.git.proxy"
        );
        assert_eq!(
            env[&format!("GIT_CONFIG_VALUE_{index}")],
            "http://dom%5Cme@proxy.corp:3128"
        );
        assert_eq!(env["CORVENE_ASKPASS_PROXIES"], "proxy.corp:3128");
        let with_user = GitProxy {
            has_user: true,
            ..proxy
        };
        assert!(credential_env(&with_user, "https://github.com", &logins).is_empty());
    }

    #[test]
    fn fallback_urls() {
        assert_eq!(
            https_url_for("git@github.com:o/r.git").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            https_url_for("ssh://git@ghe.corp:2222/o/r").as_deref(),
            Some("https://ghe.corp")
        );
        assert_eq!(https_url_for("https://x/y").as_deref(), Some("https://x/y"));
        assert_eq!(https_url_for("/srv/repo.git"), None);
        assert_eq!(https_url_for("file:///srv/repo.git"), None);
    }

    #[test]
    fn proxy_auth_failures() {
        assert!(is_proxy_auth_failure(
            "fatal: unable to access 'https://github.com/o/r/': CONNECT tunnel failed, response 407"
        ));
        assert!(is_proxy_auth_failure(
            "fatal: unable to access 'x': Received HTTP code 407 from proxy after CONNECT"
        ));
        assert!(!is_proxy_auth_failure(
            "fatal: Authentication failed for 'x'"
        ));
    }
}
