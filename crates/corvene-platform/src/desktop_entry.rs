//! Desktop integration for an AppImage (GHD ships no Linux build; Electron's
//! AppImages leave this to AppImageLauncher / appimaged).
//!
//! The `.deb` installs `com.wasimaster.corvene.desktop` and the icons under
//! `/usr/share`. An AppImage, downloaded by hand or moved to
//! `~/Applications` by the Homebrew cask, installs nothing, so there is no
//! launcher entry and [`crate::url_schemes::register`] has no entry to make
//! the `x-corvene://` / `x-corvene-auth://` handler. Every launch from an
//! AppImage (`$APPIMAGE`) therefore writes
//! `$XDG_DATA_HOME/applications/com.wasimaster.corvene.desktop` from
//! `packaging/linux/com.wasimaster.corvene.desktop` with `Exec` / `TryExec`
//! naming the image, and copies the icons from the mounted AppDir
//! (`$APPDIR/usr/share/icons`) to `$XDG_DATA_HOME/icons/hicolor`.
//!
//! Rules ([`plan`]):
//! - an entry this module wrote ([`MARKER`]) is rewritten when its contents
//!   differ (the image moved, the template changed);
//! - a user entry without the marker is somebody else's and is left alone;
//! - with no user entry, an entry in a system data directory (the `.deb`'s)
//!   wins and nothing is written. Nothing outside `$XDG_DATA_HOME` is ever
//!   touched;
//! - when another user entry already starts this image (AppImageLauncher's
//!   and appimaged's `appimagekit_*.desktop`, [`integrated_entry`]), that
//!   tool owns the launcher: nothing is written and an entry of ours is
//!   removed, so the menu never shows Corvene twice.
//!
//! A launch that is not from an AppImage removes an entry this module wrote
//! once its image is gone ([`remove_stale`]): it would shadow the `.deb`'s.
//!
//! Corvene (`434-launcher-recent-repositories`, desktop#20943's Linux
//! side; GHD has no Linux build, and on Windows it has no jump list): the
//! recently opened repositories become the entry's actions
//! (`[Desktop Action repo-N]`, listed in `Actions=`), which GNOME Shell,
//! KDE Plasma, Cinnamon and Budgie show in the launcher's context menu.
//! Each runs the entry's program with `x-corvene://openLocalRepo/<path>`,
//! the URL `corvene open <path>` sends, so a running Corvene selects the
//! repository (single instance) and a new one starts with it. Desktop
//! entries are static, so the list lives in the user's entry
//! ([`set_recent_repositories`]): the AppImage's own entry gets the actions,
//! and over a system entry (the `.deb`'s) Corvene writes a copy of it with
//! the actions and [`RECENT_MARKER`], which goes away again when there is
//! nothing to list or the flag is off ([`recent_plan`]). Flatpak and Snap
//! export their own entries from outside the sandbox: nothing is written
//! there.
//!
//! Everything but [`ensure`] and [`set_recent_repositories`] is plain path
//! and file logic that compiles (and is tested) on every OS.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "macos"))]
use std::sync::{Mutex, PoisonError};

/// The `.desktop` file's name (the GPUI `app_id` / `WM_CLASS`).
pub const DESKTOP_ID: &str = "com.wasimaster.corvene.desktop";

/// The packaged entry (`Exec=/usr/lib/corvene/corvene %U`, the `.deb`'s).
pub const TEMPLATE: &str = include_str!("../../../packaging/linux/com.wasimaster.corvene.desktop");

/// The line that marks an entry as written by this module.
pub const MARKER: &str = "X-Corvene-Installer=appimage";

/// The line that marks a user entry written over a system entry for the
/// recent repositories' actions.
pub const RECENT_MARKER: &str = "X-Corvene-Installer=recent-repositories";

/// A repository for the entry's actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentRepository {
    /// What the action is called (the repository list's name).
    pub name: String,
    /// What opens it: `x-corvene://openLocalRepo/<path>` (the core's
    /// `app_url::open_local_repo_url`, what `corvene open <path>` sends).
    pub url: String,
}

/// The `[Desktop Entry]` group's lines of `entry`: `(first, end)` line
/// indexes, the group header included, `end` exclusive.
fn main_group(lines: &[&str]) -> Option<(usize, usize)> {
    let first = lines.iter().position(|l| l.trim() == "[Desktop Entry]")?;
    let end = lines[first + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| first + 1 + i);
    Some((first, end))
}

/// The main group's `Exec` without its trailing field code (`%U`): the
/// program, as written (quoted and escaped).
pub fn exec_program(entry: &str) -> Option<String> {
    let lines: Vec<&str> = entry.lines().collect();
    let (first, end) = main_group(&lines)?;
    let exec = lines[first..end]
        .iter()
        .find_map(|l| l.strip_prefix("Exec="))?
        .trim();
    let program = match exec.rsplit_once(' ') {
        Some((program, code)) if code.len() == 2 && code.starts_with('%') => program,
        _ => exec,
    };
    (!program.is_empty()).then(|| program.to_string())
}

/// `entry` with one `[Desktop Action repo-N]` per repository, in order,
/// named in the main group's `Actions=` (after any it has). `None` when an
/// action cannot be written (no `Exec`, a name or path a key cannot hold).
pub fn with_actions(entry: &str, repositories: &[RecentRepository]) -> Option<String> {
    if repositories.is_empty() {
        return Some(entry.to_string());
    }
    let program = exec_program(entry)?;
    let mut groups = String::new();
    let mut ids = String::new();
    for (i, repository) in repositories.iter().enumerate() {
        let id = format!("repo-{}", i + 1);
        let url = exec_arg(&repository.url)?;
        let name = string_value(&repository.name)?;
        groups.push_str(&format!(
            "\n[Desktop Action {id}]\nName={name}\nExec={program} {url}\n"
        ));
        ids.push_str(&id);
        ids.push(';');
    }
    let lines: Vec<&str> = entry.lines().collect();
    let (first, mut end) = main_group(&lines)?;
    // blank lines before the next group stay after the key
    while end > first + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    let mut out = String::with_capacity(entry.len() + groups.len() + 64);
    let mut listed = false;
    for (i, line) in lines.iter().enumerate() {
        if i == end && !listed {
            out.push_str(&format!("Actions={ids}\n"));
            listed = true;
        }
        match line.strip_prefix("Actions=") {
            Some(existing) if (first..end).contains(&i) => {
                let existing = existing.trim_end();
                let sep = if existing.is_empty() || existing.ends_with(';') {
                    ""
                } else {
                    ";"
                };
                out.push_str(&format!("Actions={existing}{sep}{ids}\n"));
                listed = true;
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !listed {
        out.push_str(&format!("Actions={ids}\n"));
    }
    out.push_str(&groups);
    Some(out)
}

/// Whether `contents` is a user entry this module wrote over a system one.
pub fn is_recent_override(contents: &str) -> bool {
    contents
        .lines()
        .any(|line| line.trim_end() == RECENT_MARKER)
}

/// What to do about the user's entry for the recent repositories when
/// Corvene does not run from an AppImage ([`recent_plan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecentPlan {
    /// Write these contents.
    Write(String),
    /// Remove the override of ours: nothing to list.
    Remove,
    /// Leave the user's folder as it is.
    Nothing,
}

/// From the user's entry (`None`: there is none) and the system entry's
/// contents (`None`: there is none): an override of the system entry with
/// the actions, while there are repositories to list; never over an entry
/// that is not such an override (an AppImage's, the user's own).
pub fn recent_plan(
    user_entry: Option<&str>,
    system_entry: Option<&str>,
    repositories: &[RecentRepository],
) -> RecentPlan {
    let ours = match user_entry {
        None => false,
        Some(existing) if is_recent_override(existing) => true,
        Some(_) => return RecentPlan::Nothing,
    };
    let wanted = system_entry
        .filter(|_| !repositories.is_empty())
        .and_then(|system| {
            let mut base = system.trim_end().to_string();
            base.push('\n');
            base.push_str(RECENT_MARKER);
            base.push('\n');
            with_actions(&base, repositories)
        });
    match wanted {
        Some(wanted) if user_entry == Some(wanted.as_str()) => RecentPlan::Nothing,
        Some(wanted) => RecentPlan::Write(wanted),
        None if ours => RecentPlan::Remove,
        None => RecentPlan::Nothing,
    }
}

/// [`recent_plan`], carried out under `data_home`. `Ok(true)` when the
/// user's entry changed.
pub fn install_recent(
    data_home: &Path,
    system_entry: Option<&Path>,
    repositories: &[RecentRepository],
) -> std::io::Result<bool> {
    let path = user_entry_path(data_home);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Ok(false),
    };
    let system = system_entry.and_then(|p| std::fs::read_to_string(p).ok());
    match recent_plan(existing.as_deref(), system.as_deref(), repositories) {
        RecentPlan::Write(contents) => {
            write_atomic(&path, contents.as_bytes())?;
            Ok(true)
        }
        RecentPlan::Remove => {
            std::fs::remove_file(&path)?;
            Ok(true)
        }
        RecentPlan::Nothing => Ok(false),
    }
}

/// The icons of `packaging/linux/package.sh`'s `stage`, relative to a
/// `share` directory (the AppDir's `usr/share`, `$XDG_DATA_HOME`).
pub const ICONS: [&str; 2] = [
    "icons/hicolor/256x256/apps/com.wasimaster.corvene.png",
    "icons/hicolor/scalable/apps/com.wasimaster.corvene.svg",
];

/// The image a `$APPIMAGE` value names: an absolute path. Unlike
/// `updater::running_appimage` it need not be replaceable (an image in a
/// read-only folder still gets a launcher) and symlinks are kept.
pub fn appimage_path(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value.filter(|v| !v.is_empty())?);
    path.is_absolute().then_some(path)
}

/// `<data_home>/applications/com.wasimaster.corvene.desktop`
pub fn user_entry_path(data_home: &Path) -> PathBuf {
    data_home.join("applications").join(DESKTOP_ID)
}

/// The entry in a system data directory (`$XDG_DATA_DIRS`, the spec's
/// default when unset or empty), if any.
pub fn system_entry(data_dirs: Option<&str>) -> Option<PathBuf> {
    data_dirs
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or("/usr/local/share:/usr/share")
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join("applications").join(DESKTOP_ID))
        .find(|path| path.is_file())
}

/// A desktop entry string value: backslashes doubled. `None` for a value a
/// one-line key cannot hold.
fn string_value(text: &str) -> Option<String> {
    (!text.contains(['\n', '\r'])).then(|| text.replace('\\', "\\\\"))
}

/// `path` as the program of an `Exec` key (Desktop Entry Specification, "The
/// Exec key"): quoted when it has a reserved character, with `"`, `` ` ``,
/// `$` and `\` escaped inside the quotes, `%` doubled, and the result
/// escaped once more as a string value.
pub fn exec_arg(path: &str) -> Option<String> {
    const RESERVED: &[char] = &[
        ' ', '\t', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
    ];
    let arg = if path.contains(RESERVED) {
        let mut quoted = String::with_capacity(path.len() + 2);
        quoted.push('"');
        for c in path.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        quoted
    } else {
        path.to_string()
    };
    string_value(&arg.replace('%', "%%"))
}

/// `template` with `Exec=<image> %U`, `TryExec=<image>` (the launcher goes
/// away with the image) and [`MARKER`]. `None` when the path cannot be
/// written into an entry (not UTF-8, a line break).
pub fn entry_contents(template: &str, image: &Path) -> Option<String> {
    let image = image.to_str()?;
    let exec = exec_arg(image)?;
    let try_exec = string_value(image)?;
    let mut out = String::with_capacity(template.len() + 2 * image.len());
    for line in template.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec={exec} %U"));
        } else if line.starts_with("TryExec=") {
            out.push_str(&format!("TryExec={try_exec}"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push_str(MARKER);
    out.push('\n');
    Some(out)
}

/// Whether `contents` is an entry this module wrote.
pub fn is_ours(contents: &str) -> bool {
    contents.lines().any(|line| line.trim_end() == MARKER)
}

/// The image an entry of ours starts (its `TryExec`).
pub fn entry_image(contents: &str) -> Option<PathBuf> {
    let value = contents
        .lines()
        .find_map(|line| line.strip_prefix("TryExec="))?;
    Some(PathBuf::from(value.replace("\\\\", "\\")))
}

/// Another entry in `<data_home>/applications` whose `Exec` or `TryExec`
/// names `image`: the one AppImageLauncher or appimaged wrote for it.
pub fn integrated_entry(data_home: &Path, image: &Path) -> Option<PathBuf> {
    let image = image.to_str()?;
    let quoted = exec_arg(image)?;
    std::fs::read_dir(data_home.join("applications"))
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "desktop")
                && path.file_name().is_some_and(|name| name != DESKTOP_ID)
        })
        .find(|path| {
            std::fs::read_to_string(path).is_ok_and(|contents| {
                contents
                    .lines()
                    .filter_map(|line| {
                        line.strip_prefix("Exec=")
                            .or_else(|| line.strip_prefix("TryExec="))
                    })
                    .any(|value| value.contains(image) || value.contains(&quoted))
            })
        })
}

/// What to do about the user's entry ([`plan`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Write (or rewrite) the entry.
    Write,
    /// Our entry is already what it should be.
    UpToDate,
    /// The user's entry was not written by Corvene.
    Foreign,
    /// A system data directory has the entry (the `.deb`).
    System,
}

/// Decide from the user's entry (`None`: there is none), whether a system
/// data directory has one, and the entry [`entry_contents`] wants.
pub fn plan(user_entry: Option<&str>, system_entry: bool, wanted: &str) -> Plan {
    match user_entry {
        Some(existing) if !is_ours(existing) => Plan::Foreign,
        Some(existing) if existing == wanted => Plan::UpToDate,
        Some(_) => Plan::Write,
        None if system_entry => Plan::System,
        None => Plan::Write,
    }
}

/// Write `contents` to `path` through a temporary file and a rename.
fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!(".{name}.{}", std::process::id()));
    std::fs::write(&temp, contents)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Install the entry for `image`, with an action per `recent` repository,
/// and the icons from `app_dir` (the mounted AppDir) under `data_home`, per
/// [`plan`]. `Ok(true)` when a file changed;
/// the icons are only touched while the entry is ours. With an
/// [`integrated_entry`] for the image, an entry of ours is removed instead.
pub fn install(
    data_home: &Path,
    system_entry: bool,
    image: &Path,
    app_dir: Option<&Path>,
    recent: &[RecentRepository],
) -> std::io::Result<bool> {
    let Some(wanted) =
        entry_contents(TEMPLATE, image).and_then(|entry| with_actions(&entry, recent))
    else {
        return Ok(false);
    };
    let path = user_entry_path(data_home);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        // unreadable or not text: not ours to replace
        Err(_) => return Ok(false),
    };
    if integrated_entry(data_home, image).is_some() {
        let ours = existing.as_deref().is_some_and(is_ours);
        if ours {
            std::fs::remove_file(&path)?;
        }
        return Ok(ours);
    }
    let mut changed = false;
    match plan(existing.as_deref(), system_entry, &wanted) {
        Plan::Foreign | Plan::System => return Ok(false),
        Plan::UpToDate => {}
        Plan::Write => {
            write_atomic(&path, wanted.as_bytes())?;
            changed = true;
        }
    }
    for icon in ICONS {
        let Some(source) = app_dir.map(|dir| dir.join("usr/share").join(icon)) else {
            break;
        };
        let Ok(bytes) = std::fs::read(&source) else {
            continue;
        };
        let target = data_home.join(icon);
        if std::fs::read(&target).is_ok_and(|current| current == bytes) {
            continue;
        }
        write_atomic(&target, &bytes)?;
        changed = true;
    }
    Ok(changed)
}

/// Remove the user's entry when it is ours and its image no longer exists
/// (the AppImage was deleted; the entry would shadow the `.deb`'s).
/// `Ok(true)` when it was removed.
pub fn remove_stale(data_home: &Path) -> std::io::Result<bool> {
    let path = user_entry_path(data_home);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    if !is_ours(&contents) || entry_image(&contents).is_none_or(|image| image.exists()) {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

/// Keep the user's desktop entry in step with how this process was started
/// (blocking; run off the main thread): [`install`] for an AppImage,
/// [`remove_stale`] otherwise, then refresh the desktop's caches.
#[cfg(not(target_os = "macos"))]
pub fn ensure() {
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(data_home) = dirs::data_dir() else {
        return;
    };
    let recent = RECENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_default();
    let image = appimage_path(std::env::var_os("APPIMAGE")).filter(|image| image.is_file());
    let result = match &image {
        Some(image) => {
            let data_dirs = std::env::var("XDG_DATA_DIRS").ok();
            let app_dir = std::env::var_os("APPDIR").map(PathBuf::from);
            install(
                &data_home,
                system_entry(data_dirs.as_deref()).is_some(),
                image,
                app_dir.as_deref(),
                &recent,
            )
        }
        None => remove_stale(&data_home),
    };
    refresh_caches(&data_home, result);
}

/// The recent repositories [`ensure`] writes as actions; `None` until the
/// app hands them over (or while the flag is off).
#[cfg(not(target_os = "macos"))]
static RECENT: Mutex<Option<Vec<RecentRepository>>> = Mutex::new(None);

/// [`ensure`] and [`set_recent_repositories`] write the same file.
#[cfg(not(target_os = "macos"))]
static WRITING: Mutex<()> = Mutex::new(());

/// Set by [`manage_launcher_actions`]: until then [`set_recent_repositories`]
/// only remembers the list.
#[cfg(not(target_os = "macos"))]
static MANAGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app's own process, at startup: [`set_recent_repositories`] writes
/// the user's entry. A process that never calls this (the dispatcher's
/// tests, an isolated harness instance) leaves the user's launcher alone.
#[cfg(not(target_os = "macos"))]
pub fn manage_launcher_actions() {
    MANAGED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Corvene (`434-launcher-recent-repositories`): the repositories for the
/// entry's actions, most recent first; `None` while the flag is off
/// (the actions go away). Blocking: run off the main thread.
#[cfg(not(target_os = "macos"))]
pub fn set_recent_repositories(repositories: Option<Vec<RecentRepository>>) {
    {
        let mut recent = RECENT.lock().unwrap_or_else(PoisonError::into_inner);
        if *recent == repositories {
            return;
        }
        recent.clone_from(&repositories);
    }
    if !MANAGED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    // a sandbox's entry is exported from outside it
    if std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some() {
        return;
    }
    let repositories = repositories.unwrap_or_default();
    if appimage_path(std::env::var_os("APPIMAGE")).is_some_and(|image| image.is_file()) {
        ensure();
        return;
    }
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(data_home) = dirs::data_dir() else {
        return;
    };
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok();
    let system = system_entry(data_dirs.as_deref());
    refresh_caches(
        &data_home,
        install_recent(&data_home, system.as_deref(), &repositories),
    );
}

/// After the user's entry changed: the desktop's caches.
#[cfg(not(target_os = "macos"))]
fn refresh_caches(data_home: &Path, result: std::io::Result<bool>) {
    match result {
        Ok(false) => {}
        Ok(true) => {
            tracing::info!(entry = %user_entry_path(data_home).display(), "desktop entry updated");
            // both are optional tools; the files alone are enough for most desktops
            let _ = std::process::Command::new("update-desktop-database")
                .arg("-q")
                .arg(data_home.join("applications"))
                .status();
            let _ = std::process::Command::new("gtk-update-icon-cache")
                .args(["-q", "-t", "-f"])
                .arg(data_home.join("icons/hicolor"))
                .status();
        }
        Err(err) => tracing::warn!(%err, "could not update the desktop entry"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: &str = "/home/me/Applications/Corvene.AppImage";

    fn wanted(image: &str) -> String {
        entry_contents(TEMPLATE, Path::new(image)).unwrap()
    }

    #[test]
    fn appimage_path_needs_an_absolute_path() {
        assert_eq!(appimage_path(Some(IMAGE.into())), Some(IMAGE.into()));
        assert_eq!(appimage_path(None), None);
        assert_eq!(appimage_path(Some("".into())), None);
        assert_eq!(appimage_path(Some("Corvene.AppImage".into())), None);
    }

    #[test]
    fn exec_arg_quotes_only_when_needed() {
        assert_eq!(exec_arg(IMAGE).unwrap(), IMAGE);
        assert_eq!(
            exec_arg("/home/me/My Apps/Corvene.AppImage").unwrap(),
            "\"/home/me/My Apps/Corvene.AppImage\""
        );
        // `$` and `\` are escaped in the quotes, then once more as a string
        assert_eq!(exec_arg("/a/$b").unwrap(), r#""/a/\\$b""#);
        assert_eq!(exec_arg(r"/a/b\c").unwrap(), r#""/a/b\\\\c""#);
        assert_eq!(exec_arg(r#"/a/"b"#).unwrap(), r#""/a/\\"b""#);
        // a field code look-alike
        assert_eq!(exec_arg("/a/100%U/c").unwrap(), "/a/100%%U/c");
        assert_eq!(exec_arg("/a/b\nc"), None);
    }

    #[test]
    fn entry_points_at_the_image_and_keeps_the_rest_of_the_template() {
        let entry = wanted(IMAGE);
        let lines: Vec<&str> = entry.lines().collect();
        assert_eq!(lines[0], "[Desktop Entry]");
        assert!(lines.contains(&format!("Exec={IMAGE} %U").as_str()));
        assert!(lines.contains(&format!("TryExec={IMAGE}").as_str()));
        assert!(!entry.contains("/usr/lib/corvene"));
        assert!(
            lines.contains(&"MimeType=x-scheme-handler/x-corvene;x-scheme-handler/x-corvene-auth;")
        );
        assert!(lines.contains(&"Icon=com.wasimaster.corvene"));
        assert_eq!(lines.len(), TEMPLATE.lines().count() + 1);
        assert!(is_ours(&entry));
        assert!(!is_ours(TEMPLATE));
        assert_eq!(entry_image(&entry), Some(PathBuf::from(IMAGE)));

        let spaced = wanted("/home/me/My Apps/Corvene.AppImage");
        assert!(spaced.contains("Exec=\"/home/me/My Apps/Corvene.AppImage\" %U\n"));
        assert!(spaced.contains("TryExec=/home/me/My Apps/Corvene.AppImage\n"));
        assert_eq!(
            entry_image(&wanted(r"/a/b\c")),
            Some(PathBuf::from(r"/a/b\c"))
        );
        assert_eq!(entry_contents(TEMPLATE, Path::new("/a/b\nc")), None);
    }

    #[test]
    fn plan_only_writes_what_is_ours_or_missing() {
        let entry = wanted(IMAGE);
        assert_eq!(plan(None, false, &entry), Plan::Write);
        assert_eq!(plan(None, true, &entry), Plan::System);
        assert_eq!(plan(Some(&entry), false, &entry), Plan::UpToDate);
        assert_eq!(plan(Some(&entry), true, &entry), Plan::UpToDate);
        let moved = wanted("/opt/Corvene.AppImage");
        assert_eq!(plan(Some(&moved), false, &entry), Plan::Write);
        assert_eq!(plan(Some(TEMPLATE), false, &entry), Plan::Foreign);
    }

    /// `<tmp>/share` (data home), `<tmp>/Corvene.AppImage` and a mounted
    /// AppDir with both icons.
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let data_home = tmp.path().join("share");
        let image = tmp.path().join("Corvene.AppImage");
        std::fs::write(&image, b"image").unwrap();
        let app_dir = tmp.path().join("mount");
        for icon in ICONS {
            let path = app_dir.join("usr/share").join(icon);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, icon.as_bytes()).unwrap();
        }
        (tmp, data_home, image, app_dir)
    }

    #[test]
    fn install_writes_the_entry_and_icons_once() {
        let (tmp, data_home, image, app_dir) = setup();
        assert!(install(&data_home, false, &image, Some(&app_dir), &[]).unwrap());
        let entry = std::fs::read_to_string(user_entry_path(&data_home)).unwrap();
        assert_eq!(entry, entry_contents(TEMPLATE, &image).unwrap());
        for icon in ICONS {
            assert_eq!(
                std::fs::read(data_home.join(icon)).unwrap(),
                icon.as_bytes()
            );
        }
        // nothing to do the second time, and no temporary files are left
        assert!(!install(&data_home, false, &image, Some(&app_dir), &[]).unwrap());
        let names: Vec<_> = std::fs::read_dir(data_home.join("applications"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [DESKTOP_ID]);

        // the image moved: the entry follows it
        let moved = tmp.path().join("Corvene-0.2.0-x86_64.AppImage");
        std::fs::rename(&image, &moved).unwrap();
        assert!(install(&data_home, false, &moved, Some(&app_dir), &[]).unwrap());
        let entry = std::fs::read_to_string(user_entry_path(&data_home)).unwrap();
        assert_eq!(entry_image(&entry), Some(moved.clone()));

        // a new icon in the image replaces the installed one
        std::fs::write(app_dir.join("usr/share").join(ICONS[1]), b"new").unwrap();
        assert!(install(&data_home, false, &moved, Some(&app_dir), &[]).unwrap());
        assert_eq!(std::fs::read(data_home.join(ICONS[1])).unwrap(), b"new");
        // without a mounted AppDir the entry is still kept up to date
        assert!(!install(&data_home, false, &moved, None, &[]).unwrap());
    }

    #[test]
    fn install_leaves_foreign_and_system_entries_alone() {
        let (_tmp, data_home, image, app_dir) = setup();
        // the .deb's entry exists: nothing is written
        assert!(!install(&data_home, true, &image, Some(&app_dir), &[]).unwrap());
        assert!(!data_home.exists());

        // an entry somebody else put in the user's folder stays as it is
        let path = user_entry_path(&data_home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, TEMPLATE).unwrap();
        assert!(!install(&data_home, false, &image, Some(&app_dir), &[]).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
        assert!(!data_home.join("icons").exists());
        assert!(!remove_stale(&data_home).unwrap());
        assert!(path.exists());
    }

    #[test]
    fn another_tools_entry_for_the_image_replaces_ours() {
        let (tmp, data_home, image, app_dir) = setup();
        let applications = data_home.join("applications");
        std::fs::create_dir_all(&applications).unwrap();
        // an entry for some other image changes nothing
        let other = applications.join("appimagekit_0123-Other.desktop");
        std::fs::write(&other, "[Desktop Entry]\nExec=/opt/Other.AppImage %U\n").unwrap();
        assert_eq!(integrated_entry(&data_home, &image), None);
        assert!(install(&data_home, false, &image, Some(&app_dir), &[]).unwrap());
        assert!(user_entry_path(&data_home).exists());
        // our own entry does not count as another tool's
        assert_eq!(integrated_entry(&data_home, &image), None);

        // AppImageLauncher integrates the image: ours goes away and stays away
        let theirs = applications.join("appimagekit_4567-Corvene.desktop");
        let contents = format!(
            "[Desktop Entry]\nExec={} %U\nTryExec={}\n",
            exec_arg(image.to_str().unwrap()).unwrap(),
            image.display()
        );
        std::fs::write(&theirs, contents).unwrap();
        assert_eq!(integrated_entry(&data_home, &image), Some(theirs.clone()));
        assert!(install(&data_home, false, &image, Some(&app_dir), &[]).unwrap());
        assert!(!user_entry_path(&data_home).exists());
        assert!(!install(&data_home, false, &image, Some(&app_dir), &[]).unwrap());
        assert!(!user_entry_path(&data_home).exists());
        assert!(theirs.exists());

        // a path with a space is quoted in `Exec`
        let spaced = tmp.path().join("My Apps/Corvene.AppImage");
        std::fs::write(
            &theirs,
            format!(
                "[Desktop Entry]\nExec={} %U\n",
                exec_arg(spaced.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(integrated_entry(&data_home, &spaced), Some(theirs));
    }

    #[test]
    fn system_entry_searches_the_data_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let share = tmp.path().join("usr/share");
        let dirs = format!("{}:{}", tmp.path().join("none").display(), share.display());
        assert_eq!(system_entry(Some(&dirs)), None);
        let entry = share.join("applications").join(DESKTOP_ID);
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, TEMPLATE).unwrap();
        assert_eq!(system_entry(Some(&dirs)), Some(entry));
    }

    #[test]
    fn a_stale_entry_of_ours_is_removed() {
        let (_tmp, data_home, image, _) = setup();
        assert!(install(&data_home, false, &image, None, &[]).unwrap());
        // the image is still there
        assert!(!remove_stale(&data_home).unwrap());
        std::fs::remove_file(&image).unwrap();
        assert!(remove_stale(&data_home).unwrap());
        assert!(!user_entry_path(&data_home).exists());
        assert!(!remove_stale(&data_home).unwrap());
    }

    /// `(name, path)` pairs, with the URLs the core builds for them.
    fn repos(paths: &[(&str, &str)]) -> Vec<RecentRepository> {
        paths
            .iter()
            .map(|(name, path)| RecentRepository {
                name: name.to_string(),
                url: format!(
                    "x-corvene://openLocalRepo/{}",
                    path.trim_start_matches('/').replace(' ', "%20")
                ),
            })
            .collect()
    }

    #[test]
    fn exec_program_drops_the_field_code() {
        assert_eq!(
            exec_program(TEMPLATE).as_deref(),
            Some("/usr/lib/corvene/corvene")
        );
        assert_eq!(
            exec_program(&wanted("/home/me/My Apps/Corvene.AppImage")).as_deref(),
            Some("\"/home/me/My Apps/Corvene.AppImage\"")
        );
        assert_eq!(exec_program("[Desktop Entry]\nName=x\n"), None);
    }

    #[test]
    fn actions_open_each_repository_with_the_entrys_program() {
        let entry = with_actions(
            TEMPLATE,
            &repos(&[
                ("corvene", "/home/me/corvene"),
                ("My app", "/home/me/My app"),
            ]),
        )
        .unwrap();
        assert!(entry.starts_with(TEMPLATE.trim_end()), "{entry}");
        assert!(entry.contains("\nActions=repo-1;repo-2;\n\n[Desktop Action repo-1]\n"));
        assert!(entry.contains(
            "[Desktop Action repo-1]\nName=corvene\n\
             Exec=/usr/lib/corvene/corvene x-corvene://openLocalRepo/home/me/corvene\n"
        ));
        // `%` doubled in `Exec`
        assert!(entry.contains(
            "[Desktop Action repo-2]\nName=My app\n\
             Exec=/usr/lib/corvene/corvene x-corvene://openLocalRepo/home/me/My%%20app\n"
        ));
        // `Actions=` belongs to the main group, before any other group
        let base = "[Desktop Entry]\nExec=/x %U\nActions=new-window\n\n[Desktop Action new-window]\nName=New\nExec=/x --new\n";
        let entry = with_actions(base, &repos(&[("a", "/a")])).unwrap();
        assert!(entry.starts_with("[Desktop Entry]\nExec=/x %U\nActions=new-window;repo-1;\n\n[Desktop Action new-window]\n"), "{entry}");
        assert!(
            entry.ends_with(
                "[Desktop Action repo-1]\nName=a\nExec=/x x-corvene://openLocalRepo/a\n"
            ),
            "{entry}"
        );
        let base = "[Desktop Entry]\nExec=/x %U\n\n[Desktop Action other]\nExec=/y\n";
        let entry = with_actions(base, &repos(&[("a", "/a")])).unwrap();
        assert!(
            entry.starts_with(
                "[Desktop Entry]\nExec=/x %U\nActions=repo-1;\n\n[Desktop Action other]\n"
            ),
            "{entry}"
        );
        // no repositories: as it was; a name a key cannot hold: none
        assert_eq!(with_actions(TEMPLATE, &[]).as_deref(), Some(TEMPLATE));
        assert_eq!(with_actions(TEMPLATE, &repos(&[("a\nb", "/a")])), None);
    }

    #[test]
    fn the_override_follows_the_list_and_never_replaces_another_entry() {
        let list = repos(&[("corvene", "/home/me/corvene")]);
        let RecentPlan::Write(ours) = recent_plan(None, Some(TEMPLATE), &list) else {
            panic!("expected an override");
        };
        assert!(is_recent_override(&ours));
        assert!(!is_ours(&ours));
        assert!(ours.contains("Exec=/usr/lib/corvene/corvene %U\n"));
        assert!(ours.contains("[Desktop Action repo-1]"));
        assert_eq!(
            recent_plan(Some(&ours), Some(TEMPLATE), &list),
            RecentPlan::Nothing
        );
        // the list changed
        let more = repos(&[("b", "/b"), ("corvene", "/home/me/corvene")]);
        assert!(matches!(
            recent_plan(Some(&ours), Some(TEMPLATE), &more),
            RecentPlan::Write(_)
        ));
        // nothing to list, the package is gone: the override goes
        assert_eq!(
            recent_plan(Some(&ours), Some(TEMPLATE), &[]),
            RecentPlan::Remove
        );
        assert_eq!(recent_plan(Some(&ours), None, &list), RecentPlan::Remove);
        assert_eq!(recent_plan(None, Some(TEMPLATE), &[]), RecentPlan::Nothing);
        // no system entry (a tarball): no launcher to add to
        assert_eq!(recent_plan(None, None, &list), RecentPlan::Nothing);
        // the AppImage's entry and the user's own are left alone
        assert_eq!(
            recent_plan(Some(&wanted(IMAGE)), Some(TEMPLATE), &list),
            RecentPlan::Nothing
        );
        assert_eq!(
            recent_plan(Some(TEMPLATE), Some(TEMPLATE), &list),
            RecentPlan::Nothing
        );
    }

    #[test]
    fn install_recent_writes_and_removes_the_override() {
        let tmp = tempfile::tempdir().unwrap();
        let data_home = tmp.path().join("share");
        let system = tmp.path().join("usr/share/applications").join(DESKTOP_ID);
        std::fs::create_dir_all(system.parent().unwrap()).unwrap();
        std::fs::write(&system, TEMPLATE).unwrap();
        let list = repos(&[("corvene", "/home/me/corvene")]);
        assert!(install_recent(&data_home, Some(&system), &list).unwrap());
        let entry = std::fs::read_to_string(user_entry_path(&data_home)).unwrap();
        assert!(entry.contains("Name=corvene\n"));
        assert!(!install_recent(&data_home, Some(&system), &list).unwrap());
        assert!(install_recent(&data_home, Some(&system), &[]).unwrap());
        assert!(!user_entry_path(&data_home).exists());
    }

    #[test]
    fn the_appimages_entry_carries_the_actions() {
        let (_tmp, data_home, image, _) = setup();
        let list = repos(&[("corvene", "/home/me/corvene")]);
        assert!(install(&data_home, false, &image, None, &list).unwrap());
        let entry = std::fs::read_to_string(user_entry_path(&data_home)).unwrap();
        assert!(is_ours(&entry));
        assert!(entry.contains(&format!(
            "Exec={} x-corvene://openLocalRepo/home/me/corvene\n",
            image.display()
        )));
        // the list emptied: the entry stays, without actions
        assert!(install(&data_home, false, &image, None, &[]).unwrap());
        let entry = std::fs::read_to_string(user_entry_path(&data_home)).unwrap();
        assert!(!entry.contains("Actions="));
    }

    /// The user's real entry, as the app writes it:
    /// `packaging/linux/launcher-actions-test.sh` points `XDG_DATA_HOME` and
    /// `XDG_DATA_DIRS` at throwaway folders and sets
    /// `CORVENE_LAUNCHER_TEST` to `write` (two repositories) or `clear`.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn set_recent_repositories_keeps_the_users_entry() {
        let Ok(mode) = std::env::var("CORVENE_LAUNCHER_TEST") else {
            return;
        };
        manage_launcher_actions();
        let list = match mode.as_str() {
            "write" => repos(&[
                ("corvene", "/home/me/code/corvene"),
                ("My notes", "/home/me/My notes"),
            ]),
            _ => Vec::new(),
        };
        set_recent_repositories(Some(list));
    }
}
