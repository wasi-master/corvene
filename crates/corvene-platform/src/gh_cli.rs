//! Corvene (`529-gh-cli-account`): the token the GitHub CLI (`gh`) is
//! signed in with. GitHub Desktop never looks at `gh`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// `gh`, looked up once: `PATH`, then the Homebrew and MacPorts prefixes
/// (an app started from Finder has a short `PATH`).
fn program() -> Option<&'static PathBuf> {
    static GH: OnceLock<Option<PathBuf>> = OnceLock::new();
    GH.get_or_init(|| {
        let name = if cfg!(windows) { "gh.exe" } else { "gh" };
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        for extra in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/opt/local/bin",
            "/usr/bin",
        ] {
            dirs.push(PathBuf::from(extra));
        }
        dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
    })
    .as_ref()
}

/// `gh auth token --hostname <host> [--user <login>]`: the token `gh` uses
/// for `host` (its active account, or `login`'s). `None` when `gh` is not
/// installed or not signed in there.
pub fn token(host: &str, login: Option<&str>) -> Option<String> {
    let mut cmd = Command::new(program()?);
    cmd.args(["auth", "token", "--hostname", host]);
    if let Some(login) = login {
        cmd.args(["--user", login]);
    }
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
}
