//! `Dispatcher` half of the flags: the single write path, the dialog's
//! actions, and the live side effects core owns (the watcher, the crash
//! reports hook). `main.rs` owns the rest (menus, theme) through its
//! observer.

use crate::host::Host;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use super::{Availability, FlagId, FlagOverrides, Flags, Preset, Value, def, ids};
use crate::dispatcher::Dispatcher;
use crate::persistence::StoreExt;
use crate::state::Popup;

/// What `import_flags_json` dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Slugs this build does not know (kept out of the store).
    pub unknown: Vec<String>,
}

impl Dispatcher {
    /// The single write path (mirrors `update_settings`): edit the stored
    /// layer, re-resolve, persist, notify, then apply live side effects.
    pub fn update_flags(cx: &mut dyn Host, edit: impl FnOnce(&mut FlagOverrides)) {
        let previous = Self::state(cx).update(cx, |s, cx| {
            let previous = s.flags.clone();
            edit(&mut s.flag_overrides);
            s.flags = Flags::resolve(&s.flag_overrides, &s.flags_env);
            if let Err(err) = s.store.save_flags(&s.flag_overrides) {
                error!(?err, "could not save flags");
            }
            cx.notify();
            previous
        });
        Self::after_flags_changed(&previous, cx);
    }

    /// Set one flag. A value equal to the preset's own drops the override,
    /// so the accent bar means "differs from the preset".
    pub fn set_flag(id: FlagId, value: Value, cx: &mut dyn Host) -> Result<(), String> {
        let def = def(id);
        def.kind.validate(&value)?;
        if let Availability::BuiltIn(reason) = def.availability() {
            return Err(reason.to_string());
        }
        if Self::state(cx).read(cx).flags.is_env_locked(id) {
            return Err(format!(
                "{} is set by CORVENE_FLAGS for this session",
                def.ident()
            ));
        }
        info!(flag = %def.ident(), value = %value.render(), "flag set");
        Self::update_flags(cx, |o| {
            if def.value_for(o.preset) == &value {
                o.overrides.remove(def.slug);
            } else {
                o.overrides.insert(def.slug.to_string(), value);
            }
        });
        Ok(())
    }

    /// Back to the preset's value.
    pub fn reset_flag(id: FlagId, cx: &mut dyn Host) {
        let slug = def(id).slug;
        Self::update_flags(cx, |o| {
            o.overrides.remove(slug);
        });
    }

    /// Switch the base layer; the overrides go with it (picking "GitHub
    /// Desktop" has to give GitHub Desktop).
    pub fn apply_preset(preset: Preset, cx: &mut dyn Host) {
        info!(preset = preset.slug(), "flags preset applied");
        Self::update_flags(cx, |o| {
            o.preset = preset;
            o.overrides.clear();
        });
    }

    /// Drop every override, keep the preset.
    pub fn reset_all_flags(cx: &mut dyn Host) {
        Self::update_flags(cx, |o| o.overrides.clear());
    }

    /// The stored layer as pretty JSON (the dialog's "Copy as JSON").
    pub fn export_flags_json(cx: &dyn Host) -> String {
        serde_json::to_string_pretty(&Self::state(cx).read(cx).flag_overrides)
            .unwrap_or_else(|_| "{}".to_string())
    }

    /// The stored layer as a `CORVENE_FLAGS` spec.
    pub fn export_env_string(cx: &dyn Host) -> String {
        super::env::render(&Self::state(cx).read(cx).flag_overrides)
    }

    /// Replace the stored layer with `text` (the dialog's "Paste JSON"):
    /// unknown slugs are dropped and reported, an invalid value aborts.
    pub fn import_flags_json(text: &str, cx: &mut dyn Host) -> Result<ImportReport, String> {
        let mut parsed: FlagOverrides =
            serde_json::from_str(text.trim()).map_err(|err| format!("Not a flags JSON: {err}"))?;
        let mut report = ImportReport::default();
        let mut keep = std::collections::BTreeMap::new();
        for (slug, value) in std::mem::take(&mut parsed.overrides) {
            match super::by_slug(&slug) {
                Some(def) => {
                    def.kind
                        .validate(&value)
                        .map_err(|err| format!("{}: {err}", def.ident()))?;
                    keep.insert(slug, value);
                }
                None => report.unknown.push(slug),
            }
        }
        parsed.overrides = keep;
        Self::update_flags(cx, |o| *o = parsed);
        Ok(report)
    }

    /// Corvene › Flags… (also `CORVENE_POPUP=flags[:query]` and
    /// `x-corvene://flags?q=`).
    pub fn open_flags(query: Option<String>, cx: &mut dyn Host) {
        Self::show_popup(Popup::Flags { query }, cx);
    }

    /// The Flags dialog's Relaunch button: start Corvene again once this
    /// process has exited (the updater's path: `open -n` the bundle on
    /// macOS, the AppImage or executable on Linux), then quit.
    pub fn relaunch(cx: &mut dyn Host) {
        // Android: the activity starts itself again and ends the process
        #[cfg(target_os = "android")]
        {
            let _ = cx;
            info!("relaunching for flags");
            corvene_platform::android::relaunch();
        }
        #[cfg(not(target_os = "android"))]
        Self::relaunch_from_bundle(cx);
    }

    #[cfg(not(target_os = "android"))]
    fn relaunch_from_bundle(cx: &mut dyn Host) {
        let Some(bundle) = corvene_platform::app_location::relaunch_target() else {
            Self::show_error(
                "Could not relaunch Corvene",
                "Corvene is not running from an app bundle; quit and start it again.",
                cx,
            );
            return;
        };
        match corvene_platform::app_location::relaunch_after_exit(&bundle, std::process::id()) {
            Ok(()) => {
                info!(bundle = %bundle.display(), "relaunching for flags");
                cx.quit();
            }
            Err(err) => Self::show_error("Could not relaunch Corvene", err, cx),
        }
    }

    /// Sign in the way `307-sign-in-flow` says (the dialog's primary button,
    /// Welcome, re-authorization prompts).
    pub fn begin_sign_in(endpoint: corvene_github::Endpoint, cx: &mut dyn Host) {
        if Self::browser_sign_in_first(&endpoint, cx) {
            Self::sign_in_web_flow(endpoint, cx)
        } else {
            Self::sign_in_device_flow(endpoint, cx)
        }
    }

    /// Whether `307-sign-in-flow` starts `endpoint`'s sign-in in the browser.
    /// "auto" does on GitHub.com when the build has a client secret (GitHub
    /// refuses the web flow's token exchange without one); GitHub Enterprise
    /// keeps the device flow, as its secret lives in the keychain.
    pub fn browser_sign_in_first(endpoint: &corvene_github::Endpoint, cx: &dyn Host) -> bool {
        match Self::state(cx).read(cx).flags.text(ids::SIGN_IN_FLOW) {
            // Android: a build without the client secret cannot finish the
            // browser flow on GitHub.com, and a phone has no terminal to
            // find out why; the one-time code works. The browser flow stays
            // one link away in the dialog.
            "browser"
                if cfg!(target_os = "android")
                    && endpoint.is_dotcom()
                    && corvene_github::CLIENT_SECRET.is_none() =>
            {
                false
            }
            "browser" => true,
            "auto" => endpoint.is_dotcom() && corvene_github::CLIENT_SECRET.is_some(),
            _ => false,
        }
    }

    /// Live side effects core owns.
    fn after_flags_changed(previous: &Flags, cx: &mut dyn Host) {
        let now = Self::state(cx).read(cx).flags.clone();
        if now == *previous {
            return;
        }
        if now.bool(ids::FS_WATCHER) != previous.bool(ids::FS_WATCHER)
            || now.number(ids::FS_WATCHER_DEBOUNCE_MS)
                != previous.number(ids::FS_WATCHER_DEBOUNCE_MS)
        {
            Self::restart_watcher(cx);
        }
        Self::sync_crash_reports_setting(cx);
        if now.bool(ids::CLI_LIST_REPOSITORIES) != previous.bool(ids::CLI_LIST_REPOSITORIES) {
            Self::sync_repository_list_file(cx);
        }
        corvene_git::set_explain_missing_workdir(now.bool(ids::GIT_SPAWN_ERROR_DETAILS));
        if now.bool(ids::PROXY_CREDENTIALS) != previous.bool(ids::PROXY_CREDENTIALS) {
            Self::sync_proxy_credentials(cx);
        }
        // flags that change how diffs are read: the cached ones are stale
        let reads_diffs = |flags: &Flags| {
            [
                ids::NON_UTF8_DIFFS,
                ids::LFS_IMAGE_PREVIEWS,
                ids::LFS_TEXT_DIFF,
                ids::UTF16_DIFFS,
                ids::TABLE_FILE_DIFF,
                ids::PDF_DIFF,
                ids::MORE_IMAGE_FORMATS,
            ]
            .map(|id| flags.bool(id))
        };
        if reads_diffs(&now) != reads_diffs(previous) {
            corvene_git::text_encoding::set_decode_legacy_text(now.bool(ids::NON_UTF8_DIFFS));
            corvene_git::utf16::set_decode_utf16(now.bool(ids::UTF16_DIFFS));
            corvene_git::table_file::set_read_table_files(now.bool(ids::TABLE_FILE_DIFF));
            corvene_git::set_pdf_diffs(now.bool(ids::PDF_DIFF));
            crate::image_decode::set_more_image_formats(now.bool(ids::MORE_IMAGE_FORMATS));
            crate::diff_cache::clear_diffs();
            for id in Self::state(cx).read(cx).visible_repositories() {
                Self::load_diff(id, cx);
                Self::load_commit_diff(id, cx);
            }
        }
        if now.bool(ids::EXTRA_EDITORS) != previous.bool(ids::EXTRA_EDITORS)
            || now.bool(ids::JETBRAINS_64BIT_HIVE) != previous.bool(ids::JETBRAINS_64BIT_HIVE)
            || now.bool(ids::INTEGRATION_APP_ICONS) != previous.bool(ids::INTEGRATION_APP_ICONS)
        {
            Self::detect_integrations(cx);
        }
        if now.bool(ids::TREE_SITTER_HIGHLIGHTING) && !previous.bool(ids::TREE_SITTER_HIGHLIGHTING)
        {
            Self::load_tree_sitter_packs(cx);
        }
        if now.bool(ids::FILE_ICONS) != previous.bool(ids::FILE_ICONS) {
            Self::load_file_icon_theme(true, cx);
        }
        if now.bool(ids::LANGUAGE_EXTENSIONS) != previous.bool(ids::LANGUAGE_EXTENSIONS) {
            if now.bool(ids::LANGUAGE_EXTENSIONS) {
                Self::load_language_extensions(cx);
            } else {
                Self::unload_language_extensions(cx);
            }
        }
        for id in now.restart_pending(&Self::state(cx).read(cx).flags_at_launch) {
            warn!(flag = %id, "flag takes effect at the next launch");
        }
    }

    /// Drop the watchers and start them again for the visible repositories
    /// (or not, when `202-fs-watcher` is off).
    pub fn restart_watcher(cx: &mut dyn Host) {
        let visible = Self::state(cx).update(cx, |s, _| {
            s.watchers.clear();
            s.visible_repositories()
        });
        for id in visible {
            Self::start_watching(id, cx);
        }
    }
}
