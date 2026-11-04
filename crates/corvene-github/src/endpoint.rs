//! GitHub.com vs GitHub Enterprise Server URL layout (`lib/api.ts
//! getEndpointForRepository`, `lib/http.ts getAbsoluteUrl`,
//! `ui/lib/enterprise-validate-url.ts validateURL`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// `https://github.com` or `https://ghe.corp`
    pub web_base: String,
    /// `https://api.github.com` or `https://ghe.corp/api/v3`
    pub api_base: String,
}

/// Why an Enterprise address was refused (GHD `InvalidURLErrorName` /
/// `InvalidProtocolErrorName`); `Display` is the sign-in store's message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnterpriseAddressError {
    #[error(
        "The GitHub Enterprise instance address doesn't appear to be a valid URL. We're \
         expecting something like https://example.ghe.com."
    )]
    InvalidUrl,
    #[error(
        "Unsupported protocol. Only https is supported when authenticating with GitHub \
         Enterprise instances."
    )]
    InvalidProtocol,
}

impl Endpoint {
    pub fn github_com() -> Self {
        Self {
            web_base: "https://github.com".into(),
            api_base: "https://api.github.com".into(),
        }
    }

    /// From a user-entered enterprise address (`ghe.corp`, `https://ghe.corp/`);
    /// `None` where GHD's `validateURL` throws. See [`Endpoint::validate_enterprise`].
    pub fn enterprise(input: &str, allow_http: bool) -> Option<Self> {
        Self::validate_enterprise(input, allow_http).ok()
    }

    /// GHD `validateURL` + `getEnterpriseAPIURL`: an address without a
    /// scheme is taken as `https://`; any scheme other than `https` is
    /// refused. With `allow_http` an explicit `http://` is kept instead
    /// (flag `enterprise-plain-http`, desktop/desktop#20245).
    pub fn validate_enterprise(
        input: &str,
        allow_http: bool,
    ) -> Result<Self, EnterpriseAddressError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(EnterpriseAddressError::InvalidUrl);
        }
        let (scheme, without_scheme) = match trimmed.split_once("://") {
            Some((scheme, rest)) => (Some(scheme), rest),
            None => (None, trimmed),
        };
        let http = match scheme {
            None => false,
            Some(s) if s.eq_ignore_ascii_case("https") => false,
            Some(s) if s.eq_ignore_ascii_case("http") && allow_http => true,
            Some(s) if is_scheme(s) => return Err(EnterpriseAddressError::InvalidProtocol),
            Some(_) => return Err(EnterpriseAddressError::InvalidUrl),
        };
        let host = without_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .trim();
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err(EnterpriseAddressError::InvalidUrl);
        }
        if host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("api.github.com") {
            return Ok(Self::github_com());
        }
        let scheme = if http { "http" } else { "https" };
        let web_base = format!("{scheme}://{host}");
        Ok(Self {
            // `getEnterpriseAPIURL`: `https://api.<host>/` for a `*.ghe.com` host
            api_base: if is_ghe(&web_base) {
                get_enterprise_api_url(&web_base)
            } else {
                format!("{web_base}/api/v3")
            },
            web_base,
        })
    }

    pub fn from_api_base(api_base: &str) -> Self {
        let trimmed = api_base.trim_end_matches('/');
        if trimmed == "https://api.github.com" {
            Self::github_com()
        } else if is_ghe(api_base) {
            // `getHTMLURL`: the `*.ghe.com` site without the `api.` subdomain
            let host = url_host(api_base).unwrap_or_default();
            let host = host.strip_prefix("api.").unwrap_or(host);
            Self {
                web_base: format!("https://{host}"),
                api_base: api_base.to_string(),
            }
        } else {
            // `getHTMLURL`: `new URL(endpoint).origin`, so a port that is
            // not the scheme's default stays (GHD 3.6.8,
            // desktop/desktop#23046) and the API path goes
            let web_base = match (trimmed.split_once("://"), url_host(trimmed)) {
                (Some((scheme, _)), Some(host)) => {
                    format!("{}://{host}", scheme.to_ascii_lowercase())
                }
                _ => trimmed.trim_end_matches("/api/v3").to_string(),
            };
            Self {
                web_base,
                api_base: api_base.to_string(),
            }
        }
    }

    pub fn is_dotcom(&self) -> bool {
        self.api_base == "https://api.github.com"
    }

    pub fn host(&self) -> &str {
        self.web_base
            .trim_start_matches("https://")
            .trim_start_matches("http://")
    }

    /// GHD `getAbsoluteUrl`: `path` relative to the API base, without one
    /// leading `/` and without the `api/v3/` an Enterprise pagination link
    /// carries; joined to the base with exactly one `/`.
    pub fn api(&self, path: &str) -> String {
        let relative = path.strip_prefix('/').unwrap_or(path);
        let relative = relative.strip_prefix("api/v3/").unwrap_or(relative);
        if relative.starts_with('/') {
            // `new URL('/x', base)`: a path from the server's root
            return format!("{}{relative}", origin(&self.api_base));
        }
        let base = self.api_base.strip_suffix('/').unwrap_or(&self.api_base);
        format!("{base}/{relative}")
    }

    /// The GraphQL endpoint: `https://api.github.com/graphql`, a `*.ghe.com`
    /// API's `/graphql`, or `/api/graphql` beside an Enterprise Server's
    /// `/api/v3`.
    pub fn graphql(&self) -> String {
        let base = self.api_base.trim_end_matches('/');
        match base.strip_suffix("/api/v3") {
            Some(server) => format!("{server}/api/graphql"),
            None => format!("{base}/graphql"),
        }
    }

    pub fn web(&self, path: &str) -> String {
        format!("{}/{}", self.web_base, path.trim_start_matches('/'))
    }
}

/// `scheme://authority` of `url`.
fn origin(url: &str) -> &str {
    let start = url.find("://").map_or(0, |i| i + 3);
    match url[start..].find('/') {
        Some(i) => &url[..start + i],
        None => url,
    }
}

/// The host of an endpoint URL (`https://api.x.ghe.com/` → `api.x.ghe.com`),
/// without credentials or a port; `None` without a `scheme://`.
pub fn endpoint_host(endpoint: &str) -> Option<&str> {
    let (_, rest) = endpoint.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

/// GHD `isGHE` (`lib/endpoint-capabilities.ts`): the endpoint is on a
/// `*.ghe.com` host (GitHub Enterprise Cloud with data residency).
pub fn is_ghe(endpoint: &str) -> bool {
    endpoint_host(endpoint).is_some_and(|host| host.to_ascii_lowercase().ends_with(".ghe.com"))
}

/// JavaScript's `new URL(endpoint).host`: the host with a port other than
/// the scheme's default, without credentials; `None` without a `scheme://`.
/// Unlike `URL.host` the host keeps its case: [`Account::host`]'s keychain
/// key is taken from it.
///
/// [`Account::host`]: corvene_models::Account::host
fn url_host(endpoint: &str) -> Option<&str> {
    let (scheme, rest) = endpoint.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => Some(":443"),
        "http" => Some(":80"),
        _ => None,
    };
    let host = default_port
        .and_then(|port| authority.strip_suffix(port))
        .unwrap_or(authority);
    (!host.is_empty() && !host.starts_with(':')).then_some(host)
}

/// GHD `getEnterpriseAPIURL(endpoint)` (`lib/api.ts`): `https://api.<host>/`
/// for a `*.ghe.com` endpoint, else `https://<host>/api/v3` (`host` with a
/// port that is not the default, see `url_host`).
pub fn get_enterprise_api_url(endpoint: &str) -> String {
    let host = url_host(endpoint).unwrap_or(endpoint);
    if is_ghe(endpoint) {
        format!("https://api.{host}/")
    } else {
        format!("https://{host}/api/v3")
    }
}

/// A URL scheme (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`).
fn is_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enterprise_parsing() {
        let e = Endpoint::enterprise("https://ghe.corp/", false).unwrap();
        assert_eq!(e.web_base, "https://ghe.corp");
        assert_eq!(e.api_base, "https://ghe.corp/api/v3");
        assert_eq!(e.host(), "ghe.corp");
        assert_eq!(
            Endpoint::enterprise("github.com", false).unwrap(),
            Endpoint::github_com()
        );
        assert!(Endpoint::enterprise("   ", false).is_none());
        let plain = |allow| Endpoint::enterprise("HTTP://ghe.corp/", allow);
        // GHD `validateURL`: only https
        assert_eq!(
            Endpoint::validate_enterprise("HTTP://ghe.corp/", false),
            Err(EnterpriseAddressError::InvalidProtocol)
        );
        assert!(plain(false).is_none());
        let plain = plain(true).unwrap();
        assert_eq!(plain.web_base, "http://ghe.corp");
        assert_eq!(plain.api_base, "http://ghe.corp/api/v3");
        assert_eq!(
            Endpoint::enterprise("ghe.corp", true).unwrap().web_base,
            "https://ghe.corp"
        );
        assert_eq!(
            Endpoint::validate_enterprise("ftp://ghe.io", true),
            Err(EnterpriseAddressError::InvalidProtocol)
        );
        assert_eq!(
            Endpoint::validate_enterprise("://ghe.io", false),
            Err(EnterpriseAddressError::InvalidUrl)
        );
        assert_eq!(
            Endpoint::enterprise("HTTPS://ghe.io:9000/x", false)
                .unwrap()
                .web_base,
            "https://ghe.io:9000"
        );
    }

    #[test]
    fn ghe_com_uses_the_api_subdomain() {
        assert!(is_ghe("https://whatever.ghe.com/api/v3"));
        assert!(!is_ghe("https://ghe.corp/api/v3"));
        assert_eq!(
            get_enterprise_api_url("https://whatever.ghe.com/api/v3"),
            "https://api.whatever.ghe.com/"
        );
        // `URL.host`: a port other than the default is kept
        assert_eq!(
            get_enterprise_api_url("https://whatever.ghe.com:8443/api/v3"),
            "https://api.whatever.ghe.com:8443/"
        );
        assert_eq!(
            get_enterprise_api_url("https://user@ghe.corp:443/"),
            "https://ghe.corp/api/v3"
        );
        let e = Endpoint::enterprise("whatever.ghe.com", false).unwrap();
        assert_eq!(e.api_base, "https://api.whatever.ghe.com/");
        assert_eq!(e.web_base, "https://whatever.ghe.com");
        let back = Endpoint::from_api_base("https://api.whatever.ghe.com/");
        assert_eq!(back.web_base, "https://whatever.ghe.com");
        assert_eq!(back.host(), "whatever.ghe.com");
        assert_eq!(back.api("user"), "https://api.whatever.ghe.com/user");
    }

    #[test]
    fn round_trips_api_base() {
        let e = Endpoint::from_api_base("https://ghe.corp/api/v3");
        assert_eq!(e.web_base, "https://ghe.corp");
        // GHD 3.6.8 `getHTMLURL`: the origin, a default port dropped
        let port = Endpoint::from_api_base("https://ghe.corp:8443/api/v3");
        assert_eq!(port.web_base, "https://ghe.corp:8443");
        assert_eq!(port.web("o/r"), "https://ghe.corp:8443/o/r");
        let default = Endpoint::from_api_base("https://ghe.corp:443/api/v3");
        assert_eq!(default.web_base, "https://ghe.corp");
        let plain = Endpoint::from_api_base("http://ghe.corp:8080/api");
        assert_eq!(plain.web_base, "http://ghe.corp:8080");
        assert!(Endpoint::from_api_base("https://api.github.com").is_dotcom());
        assert!(Endpoint::from_api_base("https://api.github.com/").is_dotcom());
        assert_eq!(
            Endpoint::from_api_base("https://ghe.corp/api/v3/").web_base,
            "https://ghe.corp"
        );
        assert_eq!(
            Endpoint::github_com().api("user"),
            "https://api.github.com/user"
        );
    }

    #[test]
    fn graphql_urls() {
        assert_eq!(
            Endpoint::github_com().graphql(),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            Endpoint::from_api_base("https://ghe.corp/api/v3").graphql(),
            "https://ghe.corp/api/graphql"
        );
        assert_eq!(
            Endpoint::from_api_base("https://api.acme.ghe.com/").graphql(),
            "https://api.acme.ghe.com/graphql"
        );
    }

    #[test]
    fn api_urls_follow_get_absolute_url() {
        let ghe = Endpoint::from_api_base("https://ghe.corp/api/v3/");
        assert_eq!(
            ghe.api("/api/v3/user/repos?page=2"),
            "https://ghe.corp/api/v3/user/repos?page=2"
        );
        assert_eq!(ghe.api("//x"), "https://ghe.corp/x");
        assert_eq!(
            Endpoint::github_com().api("/repositories/1/pulls?page=2"),
            "https://api.github.com/repositories/1/pulls?page=2"
        );
    }
}
