//! Shell detection and launching - GHD `lib/shells/win32.ts` (registry keys,
//! paths and arguments copied verbatim). GHD starts the console shells
//! through `cmd /c START`; Corvene gives them a console of their own
//! directly, which is what `START` does.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows_registry::{CLASSES_ROOT, CURRENT_USER, Key, LOCAL_MACHINE};

use super::FoundShell;
use crate::windows::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW};

/// GHD `Shell` (Windows), in the enum's order; Command Prompt is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Cmd,
    PowerShell,
    PowerShellCore,
    Hyper,
    GitBash,
    Cygwin,
    Wsl,
    WindowsTerminal,
    FluentTerminal,
    Alacritty,
    Warp,
}

pub const DEFAULT_SHELL: Shell = Shell::Cmd;

const ALL: &[Shell] = &[
    Shell::Cmd,
    Shell::PowerShell,
    Shell::PowerShellCore,
    Shell::Hyper,
    Shell::GitBash,
    Shell::Cygwin,
    Shell::Wsl,
    Shell::WindowsTerminal,
    Shell::FluentTerminal,
    Shell::Alacritty,
    Shell::Warp,
];

impl Shell {
    /// The label GHD persists and shows in Options › Integrations.
    pub fn label(self) -> &'static str {
        match self {
            Shell::Cmd => "Command Prompt",
            Shell::PowerShell => "PowerShell",
            Shell::PowerShellCore => "PowerShell Core",
            Shell::Hyper => "Hyper",
            Shell::GitBash => "Git Bash",
            Shell::Cygwin => "Cygwin",
            Shell::Wsl => "WSL",
            Shell::WindowsTerminal => "Windows Terminal",
            Shell::FluentTerminal => "Fluent Terminal",
            Shell::Alacritty => "Alacritty",
            Shell::Warp => "Warp",
        }
    }

    /// GHD `parse`: unknown labels fall back to the default.
    pub fn parse(label: &str) -> Shell {
        ALL.iter()
            .copied()
            .find(|s| s.label() == label)
            .unwrap_or(DEFAULT_SHELL)
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn system_root() -> PathBuf {
    env_path("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

/// A string value of a registry key (`name` empty: the key's default value).
fn registry_string(root: &Key, key: &str, name: &str) -> Option<String> {
    root.open(key).ok()?.get_string(name).ok()
}

fn existing(path: impl Into<PathBuf>) -> Option<PathBuf> {
    let path = path.into();
    path.is_file().then_some(path)
}

fn find_powershell() -> Option<PathBuf> {
    let path = registry_string(
        LOCAL_MACHINE,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\PowerShell.exe",
        "",
    )?;
    // the value refers to %SystemRoot% unexpanded
    const VARIABLE: &str = "%SystemRoot%";
    let path = match path.get(..VARIABLE.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(VARIABLE) => {
            format!("{}{}", system_root().display(), &path[VARIABLE.len()..])
        }
        _ => path,
    };
    existing(path)
}

fn find_powershell_core() -> Option<PathBuf> {
    existing(registry_string(
        LOCAL_MACHINE,
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\pwsh.exe",
        "",
    )?)
}

fn find_hyper() -> Option<PathBuf> {
    // `"{installationPath}\app-x.x.x\Hyper.exe" "%V"`
    let command = registry_string(
        CURRENT_USER,
        r"Software\Classes\Directory\Background\shell\Hyper\command",
        "",
    )?;
    let quoted = command
        .split(['"', '\''])
        .nth(1)
        .filter(|path| !path.is_empty());
    match quoted {
        Some(path) => existing(path),
        // the launcher in the install root
        None => existing(env_path("LocalAppData")?.join(r"hyper\Hyper.exe")),
    }
}

/// GHD `findGitBash`, which reads only the machine-wide `InstallPath`.
/// Corvene also finds a per-user Git for Windows (desktop/desktop#23107):
/// its installer writes the same value under HKCU, both kinds of install
/// leave `InstallLocation` in their uninstall entry, and a portable or
/// unregistered one is found from the `git.exe` on the PATH.
pub fn find_git_bash() -> Option<PathBuf> {
    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Git_is1";
    let roots = [LOCAL_MACHINE, CURRENT_USER];
    let install_paths = roots
        .iter()
        .filter_map(|root| registry_string(root, r"SOFTWARE\GitForWindows", "InstallPath"));
    let install_locations = roots
        .iter()
        .filter_map(|root| registry_string(root, UNINSTALL, "InstallLocation"));
    install_paths
        .chain(install_locations)
        .map(PathBuf::from)
        .chain(git_install_on_path())
        .find_map(|install| existing(install.join("git-bash.exe")))
}

/// The Git for Windows folder of the `git.exe` on the PATH: `cmd\git.exe`,
/// `bin\git.exe` and `mingw64\bin\git.exe` sit one or two levels below it.
fn git_install_on_path() -> Option<PathBuf> {
    let git = git_on_path()?;
    git.ancestors()
        .skip(1)
        .take(3)
        .find(|dir| dir.join("git-bash.exe").is_file())
        .map(Path::to_path_buf)
}

fn find_cygwin() -> Option<PathBuf> {
    [
        r"SOFTWARE\Cygwin\setup",
        r"SOFTWARE\WOW6432Node\Cygwin\setup",
    ]
    .into_iter()
    .filter_map(|key| registry_string(LOCAL_MACHINE, key, "rootdir"))
    .find_map(|root| existing(Path::new(&root).join(r"bin\mintty.exe")))
}

fn find_warp() -> Option<PathBuf> {
    let key = CURRENT_USER.open(r"Software\Warp.dev\Warp").ok()?;
    if let Some(path) = key.get_string("InstallationPath").ok().and_then(existing) {
        return Some(path);
    }
    // older installers did not record the path
    [
        env_path("LocalAppData").map(|dir| dir.join(r"warp\Warp\warp.exe")),
        env_path("ProgramFiles").map(|dir| dir.join(r"Warp\warp.exe")),
        env_path("ProgramFiles(x86)").map(|dir| dir.join(r"Warp\warp.exe")),
    ]
    .into_iter()
    .flatten()
    .find_map(existing)
}

/// `wsl.exe` when `wslconfig /list` finds a distribution.
fn find_wsl() -> Option<PathBuf> {
    let system32 = system_root().join("System32");
    let wsl = existing(system32.join("wsl.exe"))?;
    let config = existing(system32.join("wslconfig.exe"))?;
    Command::new(config)
        .arg("/list")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
        .then_some(wsl)
}

fn find_alacritty() -> Option<PathBuf> {
    existing(registry_string(
        CLASSES_ROOT,
        r"Directory\Background\shell\Open Alacritty here",
        "Icon",
    )?)
}

/// The Store's execution aliases (`wt.exe`, `flute.exe`).
fn find_windows_app(alias: &str) -> Option<PathBuf> {
    let path = env_path("LocalAppData")?
        .join(r"Microsoft\WindowsApps")
        .join(alias);
    // an alias is a reparse point that `is_file` cannot follow
    path.symlink_metadata().is_ok().then_some(path)
}

/// Installed shells in GHD's order (`getAvailableShells`).
pub fn available_shells() -> Vec<FoundShell> {
    let found = |shell: Shell, path: Option<PathBuf>| {
        path.map(|path| FoundShell {
            shell,
            bundle_id: String::new(),
            path,
        })
    };
    let cmd = env_path("comspec").unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\cmd.exe"));
    [
        found(Shell::Cmd, Some(cmd)),
        found(Shell::PowerShell, find_powershell()),
        found(Shell::PowerShellCore, find_powershell_core()),
        found(Shell::Hyper, find_hyper()),
        found(Shell::GitBash, find_git_bash()),
        found(Shell::Cygwin, find_cygwin()),
        found(Shell::Warp, find_warp()),
        found(Shell::Wsl, find_wsl()),
        found(Shell::Alacritty, find_alacritty()),
        found(Shell::WindowsTerminal, find_windows_app("wt.exe")),
        found(Shell::FluentTerminal, find_windows_app("flute.exe")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// GHD `findGitOnPath`.
fn git_on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("git.exe"))
        .find(|candidate| candidate.is_file())
}

/// GHD `launch`, detached so closing Corvene leaves the terminal open.
pub fn launch(found: &FoundShell, path: &Path) -> std::io::Result<()> {
    let dir = path.to_string_lossy();
    let mut command = Command::new(&found.path);
    command
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match found.shell {
        Shell::Cmd => {
            command.creation_flags(CREATE_NEW_CONSOLE);
            // Command Prompt knows `git` even when it is not on the PATH of
            // new processes
            if let Some(git) = git_on_path() {
                let doskey = system_root().join(r"system32\doskey.exe");
                command.arg("/K").raw_arg(format!(
                    "\"{} git=^\"{}^\" $*\"",
                    doskey.display(),
                    git.display()
                ));
            }
        }
        Shell::PowerShell | Shell::Wsl => {
            command.creation_flags(CREATE_NEW_CONSOLE);
        }
        Shell::PowerShellCore => {
            command
                .creation_flags(CREATE_NEW_CONSOLE)
                .args(["-WorkingDirectory", &dir]);
        }
        Shell::Hyper => {
            command.arg(&*dir);
        }
        Shell::Alacritty => {
            command.args(["--working-directory", &dir]);
        }
        Shell::GitBash => {
            command.arg(format!("--cd={dir}"));
        }
        Shell::Cygwin => {
            // the path goes through the environment so that shell
            // metacharacters in folder names stay literal
            command
                .args([
                    "/bin/sh",
                    "-lc",
                    "cd -- \"$(cygpath -- \"$GITHUB_DESKTOP_CYGWIN_OPEN_PATH\")\" && exec bash",
                ])
                .env("GITHUB_DESKTOP_CYGWIN_OPEN_PATH", &*dir);
        }
        Shell::Warp => {
            command.arg(format!("warp://action/new_tab?path=\"{dir}\""));
        }
        Shell::WindowsTerminal => {
            command.args(["-d", "."]);
        }
        Shell::FluentTerminal => {
            command.arg("new");
        }
    }
    command.spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labels() {
        assert_eq!(Shell::parse("Git Bash"), Shell::GitBash);
        assert_eq!(Shell::parse("Windows Terminal"), Shell::WindowsTerminal);
        assert_eq!(Shell::parse("iTerm2"), Shell::Cmd);
    }

    #[test]
    fn command_prompt_is_always_available() {
        assert_eq!(available_shells()[0].shell, Shell::Cmd);
    }
}
