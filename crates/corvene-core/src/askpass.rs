//! `GIT_ASKPASS` mode: git runs this binary with the prompt as its only
//! argument (and `CORVENE_ASKPASS=1` in the environment) and reads the answer
//! from stdout. Usernames come from `CORVENE_ASKPASS_LOGINS`
//! (`host=login;host2=login2`), passwords from the macOS Keychain entry the
//! sign-in flow stored (`corvene_platform::keychain`). Anything unknown gets
//! an empty answer, which makes git fail with an authentication error that
//! the app turns into the "Authentication failed" dialog (GHD's
//! `askpass-trampoline` does the same over a local socket).

use std::collections::HashMap;

/// `Username for 'https://github.com': ` → `github.com`
/// `Password for 'https://octocat@github.com': ` → (`github.com`, `octocat`)
pub fn parse_prompt(prompt: &str) -> Option<(&'static str, String, Option<String>)> {
    let prompt = prompt.trim();
    let (kind, rest) = if let Some(rest) = prompt.strip_prefix("Username for '") {
        ("username", rest)
    } else {
        ("password", prompt.strip_prefix("Password for '")?)
    };
    let url = rest.split('\'').next()?;
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let (user, host) = match without_scheme.split_once('@') {
        Some((user, host)) => (Some(user.to_string()), host),
        None => (None, without_scheme),
    };
    let host = host.split('/').next().unwrap_or(host).to_lowercase();
    Some((kind, host, user))
}

fn logins() -> HashMap<String, String> {
    parse_logins(&std::env::var("CORVENE_ASKPASS_LOGINS").unwrap_or_default())
}

/// `host=login;host2=login2`
pub fn parse_logins(logins: &str) -> HashMap<String, String> {
    logins
        .split(';')
        .filter_map(|pair| {
            let (host, login) = pair.split_once('=')?;
            Some((host.to_lowercase(), login.to_string()))
        })
        .collect()
}

/// Answer one prompt, or `None` when Corvene knows nothing about the host.
pub fn answer(prompt: &str) -> Option<String> {
    if classify_ssh_prompt(prompt).is_some()
        && let Some(answer) = ask_app(prompt)
    {
        return answer;
    }
    let proxies = std::env::var("CORVENE_ASKPASS_PROXIES").unwrap_or_default();
    match answer_proxy(prompt, &proxies) {
        Some(answer) => answer,
        None => answer_with(prompt, logins()),
    }
}

/// Which dialog a secret question goes to (GHD `SSHKeyPassphrase`,
/// `SSHUserPassword`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyTo {
    Key,
    User,
}

/// What ssh asks through `SSH_ASKPASS` (GHD
/// `createAskpassTrampolineHandler`; the last is ssh's notice for a security
/// key, which wants no answer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshPrompt {
    Host(corvene_git::AddSshHostInfo),
    KeyPassphrase { key: String },
    UserPassword { user: String },
    UserPresence { key: String },
}

pub fn classify_ssh_prompt(prompt: &str) -> Option<SshPrompt> {
    if prompt.starts_with("The authenticity of host ") {
        return corvene_git::parse_add_ssh_host_prompt(prompt).map(SshPrompt::Host);
    }
    if let Some(rest) = prompt.strip_prefix("Enter passphrase for ") {
        // ssh: `key '<path>': `; ssh-keygen signing: `"<path>": `
        let rest = rest.strip_prefix("key ").unwrap_or(rest);
        let rest = rest.trim_end().strip_suffix(':')?;
        let key = rest.trim_matches(|c| c == '\'' || c == '"');
        return (!key.is_empty()).then(|| SshPrompt::KeyPassphrase {
            key: native_key_path(key),
        });
    }
    if let Some(user) = prompt.strip_suffix("'s password: ")
        && user.contains('@')
    {
        return Some(SshPrompt::UserPassword {
            user: user.to_string(),
        });
    }
    if let Some(rest) = prompt.strip_prefix("Confirm user presence for key ") {
        return Some(SshPrompt::UserPresence {
            key: rest.lines().next().unwrap_or(rest).trim().to_string(),
        });
    }
    None
}

/// The ssh bundled with Git for Windows names keys `/c/Users/...`.
fn native_key_path(key: &str) -> String {
    let bytes = key.as_bytes();
    if cfg!(windows)
        && bytes.len() > 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b'/'
    {
        return format!("{}:{}", &key[1..2], &key[2..]);
    }
    key.to_string()
}

/// Ask the running app ([`crate::ssh_prompts`]) and wait. `None` when no app
/// is listening; `Some(None)` when it has no answer.
pub fn ask_app(prompt: &str) -> Option<Option<String>> {
    use std::io::{Read, Write};
    let port: u16 = std::env::var("CORVENE_ASKPASS_PORT").ok()?.parse().ok()?;
    let token = std::env::var("CORVENE_ASKPASS_TOKEN").ok()?;
    let no_ui = std::env::var_os("CORVENE_ASKPASS_NO_UI").is_some();
    #[cfg(unix)]
    let parent = std::os::unix::process::parent_id();
    #[cfg(not(unix))]
    let parent = 0u32;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    let request = format!(
        "{token}\n{parent}\n{}\n{prompt}\0",
        if no_ui { "1" } else { "0" }
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).ok()?;
    let reply = String::from_utf8(reply).ok()?;
    Some(reply.strip_prefix('y').map(str::to_string))
}

/// Corvene (`528-proxy-credentials`): git asks for the password of a proxy
/// whose URL Corvene gave the saved username (`Password for
/// 'http://user@proxy:3128': `); `proxies` (`CORVENE_ASKPASS_PROXIES`)
/// names those proxies (`host:port`, comma-separated). `None` when the
/// prompt is not about one of them; else the keychain's password, if any
/// (a proxy's prompt never gets a git host's credentials).
pub fn answer_proxy(prompt: &str, proxies: &str) -> Option<Option<String>> {
    let (kind, host, user) = parse_prompt(prompt)?;
    let is_proxy = proxies
        .split(',')
        .any(|proxy| !proxy.is_empty() && proxy.trim().eq_ignore_ascii_case(&host));
    if !is_proxy {
        return None;
    }
    if kind != "password" {
        return Some(None);
    }
    let Some(user) = user else {
        return Some(None);
    };
    Some(
        corvene_platform::keychain::proxy_password(&host, &percent_decode(&user))
            .ok()
            .flatten(),
    )
}

/// git shows the proxy URL's user as written, percent escapes included.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// [`answer`] with the host → login map given (Android: the helper process
/// passes its `CORVENE_ASKPASS_LOGINS` along with the prompt).
pub fn answer_with(prompt: &str, logins: HashMap<String, String>) -> Option<String> {
    // GHD `handleSSHHostAuthenticity`: ssh's unknown host question goes to the
    // AddSSHHost dialog, which Corvene does not have yet; declining makes ssh
    // refuse the host (`Host key verification failed.`)
    if let Some(info) = corvene_git::parse_add_ssh_host_prompt(prompt) {
        tracing::info!(
            host = %info.host,
            key_type = %info.key_type,
            "declined ssh's unknown host question"
        );
        return None;
    }
    let (kind, host, user) = parse_prompt(prompt)?;
    // git names the port (`host:8443`); logins and stored credentials are
    // kept by host name alone
    let host = match host.rsplit_once(':') {
        Some((name, port)) if !logins.contains_key(&host) && port.parse::<u16>().is_ok() => {
            name.to_string()
        }
        _ => host,
    };
    match kind {
        "username" => logins.get(&host).cloned(),
        _ => {
            let login = user.or_else(|| logins.get(&host).cloned())?;
            corvene_platform::keychain::token(&host, &login)
                .ok()
                .flatten()
                .or_else(|| {
                    corvene_platform::keychain::generic_password(&host, &login)
                        .ok()
                        .flatten()
                })
                // `529-gh-cli-account`: an account borrowed from `gh`
                .or_else(|| {
                    std::env::var_os("CORVENE_ASKPASS_GH_CLI")
                        .and_then(|_| corvene_platform::gh_cli::token(&host, Some(&login)))
                })
        }
    }
}

/// Android: git runs the `corvene-askpass` helper, which cannot reach the
/// Keystore itself. It connects to `CORVENE_ASKPASS_SOCKET` (in the
/// app-private cache directory) and sends two lines, its
/// `CORVENE_ASKPASS_LOGINS` and the prompt; the answer is [`answer_with`].
#[cfg(target_os = "android")]
pub fn serve_socket() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let Some(socket) = std::env::var_os("CORVENE_ASKPASS_SOCKET") else {
        return;
    };
    let _ = std::fs::remove_file(&socket);
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(err) => {
            tracing::warn!("askpass socket: {err}");
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("askpass".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut lines = BufReader::new(&stream).lines();
                let (Some(Ok(logins)), Some(Ok(prompt))) = (lines.next(), lines.next()) else {
                    continue;
                };
                let answer = answer_with(&prompt, parse_logins(&logins)).unwrap_or_default();
                let _ = (&stream).write_all(answer.as_bytes());
            }
        });
    if let Err(err) = spawned {
        tracing::warn!("askpass thread: {err}");
    }
}

/// Entry point for `CORVENE_ASKPASS=1 corvene "<prompt>"`.
pub fn run() -> ! {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    match answer(&prompt) {
        Some(value) => {
            println!("{value}");
            std::process::exit(0);
        }
        // a cancelled ssh question: ssh stops asking instead of trying an
        // empty answer
        None if classify_ssh_prompt(&prompt).is_some() => std::process::exit(1),
        None => {
            println!();
            std::process::exit(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_ssh_questions() {
        assert_eq!(
            classify_ssh_prompt("Enter passphrase for key '/home/u/.ssh/id_ed25519': "),
            Some(SshPrompt::KeyPassphrase {
                key: "/home/u/.ssh/id_ed25519".into()
            })
        );
        assert_eq!(
            classify_ssh_prompt("Enter passphrase for \"/home/u/.ssh/sign\": "),
            Some(SshPrompt::KeyPassphrase {
                key: "/home/u/.ssh/sign".into()
            })
        );
        assert_eq!(
            classify_ssh_prompt("git@example.com's password: "),
            Some(SshPrompt::UserPassword {
                user: "git@example.com".into()
            })
        );
        assert_eq!(
            classify_ssh_prompt("Confirm user presence for key ED25519-SK SHA256:abc"),
            Some(SshPrompt::UserPresence {
                key: "ED25519-SK SHA256:abc".into()
            })
        );
        assert!(matches!(
            classify_ssh_prompt(
                "The authenticity of host 'example.com (10.0.0.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure?"
            ),
            Some(SshPrompt::Host(_))
        ));
        assert_eq!(classify_ssh_prompt("Username for 'https://github.com': "), None);
        assert_eq!(classify_ssh_prompt("Password for 'https://u@x.com': "), None);
    }

    #[test]
    fn parses_git_prompts() {
        assert_eq!(
            parse_prompt("Username for 'https://github.com': "),
            Some(("username", "github.com".into(), None))
        );
        assert_eq!(
            parse_prompt("Password for 'https://octocat@GitHub.com': "),
            Some(("password", "github.com".into(), Some("octocat".into())))
        );
        assert_eq!(parse_prompt("Enter passphrase for key '/x': "), None);
    }

    #[test]
    fn proxy_prompts_need_a_listed_proxy() {
        let proxies = "proxy.corp:3128";
        assert_eq!(
            answer_proxy("Password for 'https://octocat@github.com': ", proxies),
            None
        );
        // a listed proxy's username question gets nothing, not a login
        assert_eq!(
            answer_proxy("Username for 'http://proxy.corp:3128': ", proxies),
            Some(None)
        );
        assert_eq!(percent_decode("dom%5Cme%"), "dom\\me%");
    }

    #[test]
    fn declines_the_unknown_host_question() {
        let prompt = "The authenticity of host 'github.com (140.82.121.3)' can't be established.\n\
                      ED25519 key fingerprint is SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU.\n\
                      Are you sure you want to continue connecting (yes/no/[fingerprint])? ";
        assert_eq!(
            answer_with(prompt, parse_logins("github.com=octocat")),
            None
        );
    }

    #[test]
    fn a_port_in_the_prompt_does_not_hide_the_login() {
        let logins = parse_logins("ghe.corp=me");
        assert_eq!(
            answer_with("Username for 'https://ghe.corp:8443': ", logins).as_deref(),
            Some("me")
        );
    }
}
