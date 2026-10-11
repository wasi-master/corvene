//! OS integration: macOS and Linux (freedesktop: XDG base directories,
//! D-Bus services, `xdg-open`). Every function here is the seam for Windows
//! later.

/// `CFBundleIdentifier` of the app bundle (keychain service, notification
/// settings deep link, GPUI `app_id`).
pub const BUNDLE_ID: &str = "com.wasimaster.corvene";

pub mod accessibility;
#[cfg(target_os = "android")]
pub mod android;
pub mod app_icons;
pub mod app_location;
pub mod apps;
pub mod bookmarks;
pub mod cli;
pub mod crash_reports;
pub mod custom_integration;
pub mod desktop_file_transport;
// freedesktop `.desktop` entries
#[cfg(not(windows))]
pub mod desktop_entry;
pub mod editors;
pub mod file_hash;
pub mod file_url;
pub mod ghd_import;
pub mod image_decode;
pub mod keychain;
pub mod locale;
pub mod notifications;
pub mod proxy;
pub mod services;
pub mod shells;
#[cfg(not(any(target_os = "macos", windows)))]
pub mod single_instance;
#[cfg(windows)]
#[path = "single_instance_windows.rs"]
pub mod single_instance;
pub mod spell;
pub mod ssh_key;
pub mod trash;
pub mod updater;
pub mod url_schemes;
#[cfg(windows)]
pub mod windows;

pub mod paths {
    use std::path::{Path, PathBuf};

    pub const APP_NAME: &str = "Corvene";

    /// The per-app folder name inside the OS's data, cache and log
    /// directories: `Corvene` on macOS, `corvene` under the XDG base
    /// directories on Linux (freedesktop convention).
    #[cfg(target_os = "macos")]
    const DIR_NAME: &str = APP_NAME;
    #[cfg(windows)]
    const DIR_NAME: &str = APP_NAME;
    #[cfg(not(any(target_os = "macos", windows)))]
    const DIR_NAME: &str = "corvene";

    fn home() -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    }

    /// `~/Library/Application Support/Corvene` (Linux: `$XDG_DATA_HOME/corvene`,
    /// `~/.local/share/corvene`), or `CORVENE_DATA_DIR` (an isolated store for
    /// test harnesses such as `tools/parity`).
    pub fn app_support_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("CORVENE_DATA_DIR") {
            return PathBuf::from(dir);
        }
        dirs::data_dir()
            .unwrap_or_else(|| home().join(".local/share"))
            .join(DIR_NAME)
    }

    /// Corvene `522-settings-file`: the read-only settings file,
    /// `~/.config/corvene/settings.json` on macOS and Linux
    /// (`%APPDATA%\corvene\settings.json` on Windows), or `settings.json`
    /// in `CORVENE_DATA_DIR` when that is set, so a harness instance never
    /// reads the user's.
    pub fn settings_file() -> PathBuf {
        if let Some(dir) = std::env::var_os("CORVENE_DATA_DIR") {
            return PathBuf::from(dir).join("settings.json");
        }
        #[cfg(windows)]
        {
            dirs::config_dir()
                .unwrap_or_else(home)
                .join("corvene")
                .join("settings.json")
        }
        #[cfg(not(windows))]
        {
            home().join(".config/corvene/settings.json")
        }
    }

    /// `~/Library/Logs/Corvene` (Linux: `$XDG_STATE_HOME/corvene/logs`,
    /// `~/.local/state/corvene/logs`; under `CORVENE_DATA_DIR` when set so a
    /// harness instance keeps its logs to itself).
    pub fn logs_dir() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            home().join("Library/Logs").join(DIR_NAME)
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(dir) = std::env::var_os("CORVENE_DATA_DIR") {
                return PathBuf::from(dir).join("logs");
            }
            state_dir().join("logs")
        }
    }

    /// `$XDG_STATE_HOME/corvene` (`~/.local/state/corvene`): logs and crash
    /// reports, which the XDG spec files under state rather than data.
    #[cfg(not(any(target_os = "macos", windows)))]
    pub fn state_dir() -> PathBuf {
        dirs::state_dir()
            .unwrap_or_else(|| home().join(".local/state"))
            .join(DIR_NAME)
    }

    /// Windows: `%LOCALAPPDATA%\Corvene` for logs and crash reports. The
    /// settings and the store are in `%APPDATA%\Corvene`
    /// ([`app_support_dir`]), where Electron keeps GitHub Desktop's.
    #[cfg(windows)]
    pub fn state_dir() -> PathBuf {
        dirs::data_local_dir().unwrap_or_else(home).join(DIR_NAME)
    }

    /// `~/Library/Caches/Corvene` (Linux: `$XDG_CACHE_HOME/corvene`,
    /// `~/.cache/corvene`; Windows: `%LOCALAPPDATA%\Corvene\Cache`)
    pub fn cache_dir() -> PathBuf {
        #[cfg(windows)]
        {
            state_dir().join("Cache")
        }
        #[cfg(not(windows))]
        {
            dirs::cache_dir()
                .unwrap_or_else(|| home().join(".cache"))
                .join(DIR_NAME)
        }
    }

    /// GitHub Desktop's default clone location: `~/Documents/GitHub`.
    /// Android: `files/repositories` in the app-private storage.
    pub fn default_clone_dir() -> PathBuf {
        default_clone_dir_avoiding_onedrive(false)
    }

    /// [`default_clone_dir`]; with `avoid_onedrive`
    /// (`515-clone-dir-avoids-onedrive`, Windows) `%USERPROFILE%\GitHub` when
    /// Documents is synced by OneDrive, which handles Git repositories badly.
    pub fn default_clone_dir_avoiding_onedrive(avoid_onedrive: bool) -> PathBuf {
        #[cfg(target_os = "android")]
        {
            let _ = avoid_onedrive;
            crate::android::repositories_dir()
        }
        #[cfg(not(target_os = "android"))]
        {
            let home = dirs::home_dir();
            match (dirs::document_dir(), home) {
                (Some(documents), Some(home)) if avoid_onedrive && cfg!(windows) => {
                    clone_dir_beside_onedrive(&documents, &home)
                }
                (documents, home) => documents
                    .or(home)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("GitHub"),
            }
        }
    }

    /// `<documents>\GitHub`, or `<home>\GitHub` when a folder on the way to
    /// Documents is OneDrive's (`OneDrive`, `OneDrive - Contoso`).
    pub fn clone_dir_beside_onedrive(documents: &Path, home: &Path) -> PathBuf {
        let onedrive = documents.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|name| name == "OneDrive" || name.starts_with("OneDrive - "))
        });
        if onedrive { home } else { documents }.join("GitHub")
    }

    #[cfg(test)]
    #[test]
    fn onedrive_documents_clone_into_the_home_folder() {
        let home = Path::new("/Users/mona");
        assert_eq!(
            clone_dir_beside_onedrive(Path::new("/Users/mona/OneDrive/Documents"), home),
            home.join("GitHub")
        );
        assert_eq!(
            clone_dir_beside_onedrive(Path::new("/Users/mona/OneDrive - Contoso/Documents"), home),
            home.join("GitHub")
        );
        assert_eq!(
            clone_dir_beside_onedrive(Path::new("/Users/mona/Documents"), home),
            Path::new("/Users/mona/Documents/GitHub")
        );
    }

    /// Candidate clone locations offered during onboarding, existing ones only
    /// except the GHD default which is always offered.
    pub fn clone_dir_candidates() -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut out = vec![default_clone_dir()];
        for candidate in [home.join("Developer"), home.join("Work")] {
            if candidate.is_dir() {
                out.push(candidate);
            }
        }
        out
    }
}

pub mod fonts {
    /// The family GitHub Desktop's `--font-family-monospace` stack
    /// (`SFMono-Regular, Consolas, "Liberation Mono", Menlo, monospace`)
    /// resolves to: SF Mono only when it is installed as a regular font
    /// (Apple's download puts `SF-Mono-*.otf` in a Fonts folder; the copy
    /// inside Terminal.app is not visible to Chromium), otherwise Menlo.
    #[cfg(target_os = "macos")]
    pub fn ghd_monospace_family() -> String {
        let dirs = [
            Some(std::path::PathBuf::from("/Library/Fonts")),
            dirs::home_dir().map(|h| h.join("Library/Fonts")),
        ];
        let installed = dirs.iter().flatten().any(|dir| {
            std::fs::read_dir(dir).is_ok_and(|entries| {
                entries.flatten().any(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with("SF-Mono") || name.starts_with("SFMono")
                })
            })
        });
        if installed { "SF Mono" } else { "Menlo" }.to_string()
    }

    /// Linux Chromium resolves the same stack through fontconfig: the first
    /// named family that is installed as itself (fontconfig substitutes only
    /// for the generic `monospace`), else what `monospace` matches
    /// (DejaVu Sans Mono on a stock Ubuntu).
    #[cfg(not(any(target_os = "macos", target_os = "android", windows)))]
    pub fn ghd_monospace_family() -> String {
        [
            "SFMono-Regular",
            "SF Mono",
            "Consolas",
            "Liberation Mono",
            "Menlo",
        ]
        .into_iter()
        .find(|family| fc_match(family).is_some_and(|m| m.eq_ignore_ascii_case(family)))
        .map(str::to_string)
        .or_else(|| fc_match("monospace"))
        .unwrap_or_else(|| "DejaVu Sans Mono".to_string())
    }

    /// The family GHD's `--font-family-sans-serif` stack (`system-ui,
    /// -apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans", Helvetica,
    /// Arial, sans-serif, …`) resolves to in Chromium on Linux: `system-ui`
    /// is the desktop's UI font (GTK's `gtk-font-name`, fontconfig's
    /// `sans-serif` without a desktop), and the named families after it only
    /// count when installed as themselves.
    #[cfg(not(any(target_os = "macos", target_os = "android", windows)))]
    pub fn ghd_ui_family() -> String {
        fc_match("sans-serif")
            .or_else(|| {
                ["Noto Sans", "Helvetica", "Arial"]
                    .into_iter()
                    .find(|family| fc_match(family).is_some_and(|m| m.eq_ignore_ascii_case(family)))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "DejaVu Sans".to_string())
    }

    /// Whether Chromium would draw text with subpixel antialiasing: its
    /// Linux font render params come from GTK, whose `gtk-xft-rgba` is the
    /// desktop's XSETTINGS (GNOME: `font-antialiasing` 'rgba') or the
    /// `Xft.rgba` X resource, and "none" (grayscale) otherwise, unless
    /// fontconfig's own `rgba` for the UI font decides first.
    #[cfg(not(any(target_os = "macos", target_os = "android", windows)))]
    pub fn subpixel_antialiasing() -> bool {
        if let Some(subpixel) = fontconfig_subpixel(&ghd_ui_family()) {
            return subpixel;
        }
        let gnome = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "font-antialiasing"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .trim_matches('\'')
                    .to_string()
            });
        if let Some(mode) = gnome {
            return mode == "rgba";
        }
        std::process::Command::new("xrdb")
            .arg("-query")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .is_some_and(|resources| {
                resources.lines().any(|line| {
                    line.strip_prefix("Xft.rgba:")
                        .is_some_and(|v| matches!(v.trim(), "rgb" | "bgr" | "vrgb" | "vbgr"))
                })
            })
    }

    /// Chromium on Windows takes the first installed family of the stack:
    /// Consolas ships with every Windows.
    #[cfg(windows)]
    pub fn ghd_monospace_family() -> String {
        "Consolas".to_string()
    }

    /// `system-ui` on Windows is Segoe UI.
    #[cfg(windows)]
    pub fn ghd_ui_family() -> String {
        "Segoe UI".to_string()
    }

    /// Chromium draws subpixel text when ClearType is on.
    #[cfg(windows)]
    pub fn subpixel_antialiasing() -> bool {
        crate::windows::cleartype()
    }

    /// Android's `monospace` family (`/system/etc/fonts.xml`): what Chrome
    /// for Android resolves the stack to, none of its named families being
    /// installed.
    #[cfg(target_os = "android")]
    pub fn ghd_monospace_family() -> String {
        "Droid Sans Mono".to_string()
    }

    /// Android's `system-ui`.
    #[cfg(target_os = "android")]
    pub fn ghd_ui_family() -> String {
        "Roboto".to_string()
    }

    /// Phone and tablet panels rotate and come in PenTile layouts; Android
    /// antialiases text in grayscale.
    #[cfg(target_os = "android")]
    pub fn subpixel_antialiasing() -> bool {
        false
    }

    /// The family Chromium draws an emoji presentation sequence with:
    /// fontconfig's `emoji` match (Noto Color Emoji on a stock desktop).
    #[cfg(not(any(target_os = "macos", target_os = "android", windows)))]
    pub fn emoji_family() -> String {
        fc_match("emoji").unwrap_or_else(|| "Noto Color Emoji".to_string())
    }

    /// Windows ships Segoe UI Emoji.
    #[cfg(windows)]
    pub fn emoji_family() -> String {
        "Segoe UI Emoji".to_string()
    }

    /// Android's emoji font (`/system/etc/fonts.xml`).
    #[cfg(target_os = "android")]
    pub fn emoji_family() -> String {
        "Noto Color Emoji".to_string()
    }

    /// fontconfig's `rgba` for the family (`fc-match -f '%{rgba}'`), which
    /// Chromium lets override GTK's render params: `Some(true)` for a
    /// subpixel layout (rgb, bgr, vrgb, vbgr), `Some(false)` for none, and
    /// `None` when fontconfig leaves it unknown.
    #[cfg(not(any(target_os = "macos", target_os = "android", windows)))]
    fn fontconfig_subpixel(family: &str) -> Option<bool> {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{rgba}", family])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        match String::from_utf8_lossy(&out.stdout).trim() {
            "1" | "2" | "3" | "4" | "rgb" | "bgr" | "vrgb" | "vbgr" => Some(true),
            "5" | "none" => Some(false),
            _ => None,
        }
    }

    /// `fc-match -f '%{family[0]}' <pattern>`: the family fontconfig picks.
    #[cfg(not(any(target_os = "macos", target_os = "android", windows)))]
    fn fc_match(pattern: &str) -> Option<String> {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{family[0]}", pattern])
            .output()
            .ok()?;
        let family = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !family.is_empty()).then_some(family)
    }
}
