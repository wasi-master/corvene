//! Corvene entry point: logging, persisted settings, GPUI application, window.
// Windows: a release build is a GUI program (no console window of its own;
// a debug build keeps one for its log)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

pub(crate) use corvene_core::askpass;
mod assets;
#[cfg(windows)]
mod cli_windows;
mod dev_samples;
mod logging;
mod menus;
#[cfg(feature = "snapshots")]
mod parity_control;

use std::sync::Arc;
use std::time::Instant;

use corvene_core::menu_state::MenuId;
use corvene_core::{Dispatcher, Popup, Section, StoreExt, ThemeSetting};
use corvene_ui::actions::*;
use corvene_ui::workspace::Workspace;
use gpui_kit::*;
use tracing::{debug, error, info, warn};

// `pub(crate)`: on Android this file is a module of the activity's native
// library (`android.rs`), whose `android_main` calls it
pub(crate) fn main() {
    // `corvene --verify-grammar <library>`: load a grammar library built by a
    // language extension and exit 0 when it reads (run as a helper process
    // by corvene_core::extensions, so a library that crashes on load never
    // takes the app down)
    {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some("--verify-grammar") {
            let Some(path) = args.next() else {
                eprintln!("usage: corvene --verify-grammar <library>");
                std::process::exit(2);
            };
            match corvene_highlight::treesitter::verify_library(std::path::Path::new(&path)) {
                Ok(names) => {
                    println!("{}", names.join("\n"));
                    std::process::exit(0);
                }
                Err(err) => {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
        }
    }
    // a stand-in git hook runs this same binary (`corvene_git::hooks`):
    // proxy the hook and exit before touching GPUI
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() == Some(std::ffi::OsStr::new(corvene_git::hooks::PROXY_ARG)) {
            let hook = args.next().unwrap_or_default();
            let rest: Vec<std::ffi::OsString> = args.collect();
            std::process::exit(corvene_git::hooks::run_proxy(&hook, &rest));
        }
    }
    // `GIT_ASKPASS` runs this same binary; answer git and exit before touching GPUI.
    if std::env::var_os("CORVENE_ASKPASS").is_some() {
        askpass::run();
    }
    // GHD `requestSingleInstanceLock`: a second launch (the `.desktop` file's
    // URL handler, the command line tool) hands its URLs to the running
    // Corvene and exits before touching the store it holds. Android keeps a
    // single activity itself (`launchMode="singleTask"`).
    #[cfg(not(target_os = "android"))]
    let launch_urls = corvene_platform::single_instance::url_arguments(std::env::args().skip(1));
    // Windows: what the command line tool asks for (`corvene.bat`) is one more URL
    #[cfg(windows)]
    let launch_urls = {
        let mut urls = launch_urls;
        urls.extend(cli_windows::url(std::env::args().skip(1)));
        urls
    };
    #[cfg(not(target_os = "android"))]
    let instance = match corvene_platform::single_instance::claim(&launch_urls) {
        corvene_platform::single_instance::Claim::Forwarded => return,
        corvene_platform::single_instance::Claim::First(listener) => listener,
    };
    let started = Instant::now();
    let pre_main_ms = since_process_start_ms();
    // the `git --version` probes run while the store, GPUI and the window
    // come up (`Dispatcher::init` collects the result)
    Dispatcher::prefetch_git();
    let _log_guard = logging::init();
    // `pre_main_ms`: from the process's start to here (dyld, the binary's
    // pages read from disk, static initialisers), what a cold launch adds
    // before any phase below
    info!(
        version = env!("CARGO_PKG_VERSION"),
        pre_main_ms, "starting corvene"
    );
    // writes a local report only while "Save crash reports locally" is on
    corvene_platform::crash_reports::install_panic_hook(env!("CARGO_PKG_VERSION"));
    phase(started, "logging initialised");

    // the store opens on a thread while GPUI creates the application (Metal
    // device, shaders, menus): neither needs the other
    let store_thread = std::thread::Builder::new()
        .name("store-open".into())
        .spawn(move || open_store(started))
        .expect("spawn the store thread");

    #[cfg(not(target_os = "android"))]
    let app = gpui_kit::application().with_assets(assets::Assets);
    // gpui-kit leaves the platform to mobile applications
    #[cfg(target_os = "android")]
    let app = Application::with_platform(gpui_android::current_platform(crate::android_app()))
        .with_assets(assets::Assets);
    phase(started, "application created");
    // `app.on('activate')`: the Dock icon shows the hidden window again.
    #[cfg(target_os = "macos")]
    app.on_reopen(|cx| {
        for handle in cx.windows() {
            handle
                .update(cx, |_, window, cx| {
                    corvene_ui::native_window::show_window(window, cx)
                })
                .ok();
        }
    });

    // `x-corvene://` URLs (the `corvene` command line tool, Finder's "Open
    // in Corvene", links) may arrive before launch has finished: queue them
    let url_inbox = corvene_core::app_url::AppUrlInbox::default();
    let url_sender = url_inbox.sender();
    #[cfg(not(target_os = "android"))]
    {
        for url in launch_urls {
            url_sender.send(url);
        }
        let from_later_launches = url_sender.clone();
        instance.serve(move |message| match message {
            corvene_platform::single_instance::Message::Url(url) => from_later_launches.send(url),
            corvene_platform::single_instance::Message::Focus => from_later_launches.focus(),
        });
        // GHD `setAsDefaultProtocolClient` on every launch (macOS: the
        // bundle's Info.plist)
        #[cfg(not(target_os = "macos"))]
        std::thread::spawn(corvene_platform::url_schemes::register);
    }
    // Windows: what the installer relies on (packaging/windows/corvene.iss).
    // The running mutex keeps the uninstaller from pulling files from under
    // a running Corvene, the restart registration lets Setup start Corvene
    // again after it closed it to replace its files, and the toast
    // activator answers clicks on notifications, also ones that started
    // this process (COM's `-Embedding`, no URL of ours).
    #[cfg(windows)]
    if std::env::var_os("CORVENE_DATA_DIR").is_none() {
        corvene_platform::windows::hold_running_mutex();
        corvene_platform::windows::register_application_restart();
        corvene_platform::notifications::serve_activator();
    }
    app.on_open_urls(move |urls| {
        for url in urls {
            url_sender.send(url);
        }
    });

    app.run(move |cx| {
        phase(started, "platform ready");
        let LaunchStore {
            store,
            settings,
            flag_overrides,
            flags_env,
            launch_flags,
            settings_file,
            store_fallback,
        } = store_thread
            .join()
            .expect("the store thread does not panic");
        sync_renderer_flags(&launch_flags);
        phase(started, "store ready");
        // the kit theme below takes the monospace family off macOS
        corvene_ui::theme::set_mono_font(corvene_platform::fonts::ghd_monospace_family().leak());
        corvene_ui::theme::preseed_kit_theme(cx);
        gpui_kit::init(cx);
        phase(started, "gpui-kit initialised");

        // CORVENE_THEME=light|dark|high-contrast overrides the saved setting (dev convenience).
        let stored_theme = settings.theme;
        let theme_setting = match std::env::var("CORVENE_THEME").as_deref() {
            Ok("light") => ThemeSetting::Light,
            Ok("dark") => ThemeSetting::Dark,
            Ok("high-contrast") => ThemeSetting::HighContrast,
            _ => settings.theme,
        };
        // `101-high-contrast-theme` off: High Contrast shows as Dark
        let high_contrast = launch_flags.bool(corvene_core::flags::ids::HIGH_CONTRAST_THEME);
        // GHD `App.render`: the welcome flow is always drawn in the light theme
        let welcome_done = settings.welcome_completed;
        let shown_theme = if welcome_done {
            theme_setting
        } else {
            ThemeSetting::Light
        };
        APPLIED_THEME.with(|t| t.set(shown_theme));
        // View › Zoom: every size in corvene-ui scales by this factor
        // (CORVENE_ZOOM=<factor> overrides the saved one for a session)
        let zoom = std::env::var("CORVENE_ZOOM")
            .ok()
            .and_then(|z| z.parse::<f32>().ok())
            .unwrap_or(settings.window_zoom_factor);
        corvene_ui::theme::sizes::set_zoom_factor(zoom);
        info!(zoom, "window zoom factor");
        let theme_variants = corvene_ui::theme::ThemeVariants::of(&launch_flags);
        corvene_ui::init(
            cx,
            resolve_theme_with(shown_theme, high_contrast, theme_variants, cx),
        );
        let sidebar_width = corvene_ui::theme::sizes::zpx(settings.sidebar_width);
        // GPUI quits through `exit`, which never drops the store: close it
        // once the windows are gone so the next launch needs no repair
        // git keeps running when Corvene quits; a command stuck on a
        // cloud-only file would wait forever, so stop what is still running
        let quit_store = store.clone();
        cx.on_app_quit(move |_| {
            corvene_git::terminate_all();
            let store = quit_store.clone();
            async move { store.close() }
        })
        .detach();
        Dispatcher::init(store, settings, flag_overrides, flags_env, cx);
        phase(started, "state initialised");
        if let Some((overlay, file_flags, errors)) = settings_file {
            Dispatcher::set_settings_file(overlay, file_flags, errors, cx);
        }
        if let Some(path) = store_fallback {
            Dispatcher::set_banner(corvene_core::Banner::TemporaryStore { path }, cx);
        }
        let state = corvene_core::AppState::global(cx);
        Dispatcher::load_custom_emoji(cx);
        // a notification click brings the (possibly hidden) window forward
        // and opens its dialog; installed before the first frame so a click
        // that launched Corvene is delivered too
        Dispatcher::listen_for_notification_clicks(focus_main_window_host, cx);
        let service_urls = url_inbox.sender();
        corvene_platform::services::register_open_in_corvene(move |path| {
            service_urls.send(corvene_core::app_url::open_local_repo_url(&path));
        });
        Dispatcher::listen_for_app_urls(url_inbox, focus_main_window_host, cx);
        #[cfg(not(target_os = "android"))]
        Dispatcher::serve_ssh_prompts(cx);
        // flag-dependent key bindings and `keymap.json`, before the menu bar
        // reads its shortcuts
        Dispatcher::load_keymap_overrides(cx);
        let keymap_flags = corvene_ui::keymap::KeymapFlags::from_flags(&state.read(cx).flags);
        let keymap_overrides = state.read(cx).keymap_overrides.clone();
        if let Some(errors) = corvene_ui::keymap::sync(keymap_flags, &keymap_overrides, cx)
            && !errors.is_empty()
        {
            Dispatcher::report_config_file_errors(corvene_core::keymap_file::path(), errors, cx);
        }
        phase(started, "theme, keymap and state installed");
        // the menu bar Linux and Windows draw in the window is there from
        // its first frame; macOS builds the system one after it (below)
        #[cfg(not(target_os = "macos"))]
        {
            let options = menus::MenuOptions::of(state.read(cx));
            menus::install(cx, &options);
        }

        // Settings › Appearance and Integrations feed back into the theme and
        // the "Open in …" menu labels; system appearance flips the System theme.
        // compared with the stored setting, so a CORVENE_THEME override holds
        // until the user picks a theme
        let mut last_theme = stored_theme;
        let mut chosen_theme = theme_setting;
        let mut last_welcome_done = welcome_done;
        // the menu bar and the theme also depend on flags (401, 101)
        let mut last_menu_key = menus::MenuOptions::of(state.read(cx));
        let mut last_high_contrast = high_contrast;
        let mut last_theme_variants = theme_variants;
        corvene_ui::format::sync(&state.read(cx).settings);
        corvene_ui::relative_time::set_calendar_dates(
            state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::CALENDAR_RELATIVE_DATES),
        );
        corvene_highlight::cm::modes::set_extra_extensions(
            state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::MORE_HIGHLIGHT_EXTENSIONS),
        );
        corvene_ui::widgets::sync_hover_while_typing(cx);
        // `428-menu-bar-status-item`
        #[cfg(target_os = "macos")]
        corvene_ui::status_item::sync(state.read(cx).menu_bar_model().as_ref(), cx);
        let mut last_watched = state.read(cx).watched_repositories();
        let mut last_reduce_motion_flag = sync_reduce_motion(cx);
        let mut last_keyboard_nav_flag = sync_keyboard_navigation(cx);
        let mut last_preferences_open = false;
        let mut last_keymap_file_flag = state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::KEYMAP_OVERRIDES);
        cx.observe(&state, move |state, cx| {
            corvene_ui::widgets::sync_hover_while_typing(cx);
            // `428-menu-bar-status-item`: the item follows the model; a
            // newly watched repository gets its status at once
            #[cfg(target_os = "macos")]
            corvene_ui::status_item::sync(state.read(cx).menu_bar_model().as_ref(), cx);
            let watched = state.read(cx).watched_repositories();
            if watched != last_watched {
                last_watched = watched;
                Dispatcher::refresh_indicators_if_stale(cx);
                Dispatcher::touch_menu_bar_statuses(cx);
            }
            let reduce_motion_flag = state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::SYSTEM_REDUCE_MOTION);
            if reduce_motion_flag != last_reduce_motion_flag {
                last_reduce_motion_flag = sync_reduce_motion(cx);
            }
            let keyboard_nav_flag = state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::SYSTEM_KEYBOARD_NAVIGATION);
            if keyboard_nav_flag != last_keyboard_nav_flag {
                last_keyboard_nav_flag = sync_keyboard_navigation(cx);
            }
            Dispatcher::sync_crash_reports_setting(cx);
            // accounts or Settings › Notifications changed: (un)subscribe
            Dispatcher::sync_alive_subscriptions(cx);
            // `618-keymap-overrides`: `keymap.json` is read again when
            // Settings closes or the flag changes
            let (preferences_open, keymap_file_flag) = {
                let s = state.read(cx);
                (
                    matches!(s.popup(), Some(Popup::Preferences { .. })),
                    s.flags.bool(corvene_core::flags::ids::KEYMAP_OVERRIDES),
                )
            };
            if (last_preferences_open && !preferences_open)
                || keymap_file_flag != last_keymap_file_flag
            {
                Dispatcher::load_keymap_overrides(cx);
            }
            last_preferences_open = preferences_open;
            last_keymap_file_flag = keymap_file_flag;
            let (theme, welcome_done, menu_key, high_contrast, variants, keymap_flags) = {
                let s = state.read(cx);
                corvene_ui::format::sync(&s.settings);
                corvene_ui::relative_time::set_calendar_dates(
                    s.flags
                        .bool(corvene_core::flags::ids::CALENDAR_RELATIVE_DATES),
                );
                corvene_highlight::cm::modes::set_extra_extensions(
                    s.flags
                        .bool(corvene_core::flags::ids::MORE_HIGHLIGHT_EXTENSIONS),
                );
                sync_renderer_flags(&s.flags);
                sync_store_flags(&s.store, &s.flags);
                (
                    s.settings.theme,
                    s.settings.welcome_completed,
                    menus::MenuOptions::of(s),
                    s.flags.bool(corvene_core::flags::ids::HIGH_CONTRAST_THEME),
                    corvene_ui::theme::ThemeVariants::of(&s.flags),
                    corvene_ui::keymap::KeymapFlags::from_flags(&s.flags),
                )
            };
            // a rebuilt keymap changes the menus' shortcuts
            let keymap_overrides = state.read(cx).keymap_overrides.clone();
            let keymap_errors = corvene_ui::keymap::sync(keymap_flags, &keymap_overrides, cx);
            let keymap_changed = keymap_errors.is_some();
            if let Some(errors) = keymap_errors.filter(|e| !e.is_empty()) {
                Dispatcher::report_config_file_errors(
                    corvene_core::keymap_file::path(),
                    errors,
                    cx,
                );
            }
            if menu_key != last_menu_key || keymap_changed {
                last_menu_key = menu_key;
                menus::install(cx, &last_menu_key);
            }
            let theme_changed = theme != last_theme;
            if theme_changed {
                last_theme = theme;
                chosen_theme = theme;
            }
            let high_contrast_changed =
                high_contrast != last_high_contrast || variants != last_theme_variants;
            last_high_contrast = high_contrast;
            last_theme_variants = variants;
            if theme_changed || high_contrast_changed || welcome_done != last_welcome_done {
                last_welcome_done = welcome_done;
                apply_theme(
                    if welcome_done {
                        chosen_theme
                    } else {
                        ThemeSetting::Light
                    },
                    cx,
                );
            }
        })
        .detach();

        // `418-confirm-quit-while-busy`: a running clone, push / pull /
        // fetch or update asks first; Quit again while it asks quits.
        cx.on_action(|_: &Quit, cx| {
            let s = corvene_core::AppState::global(cx).read(cx);
            let busy = s
                .flags
                .bool(corvene_core::flags::ids::CONFIRM_QUIT_WHILE_BUSY)
                .then(|| s.busy_for_quit())
                .flatten()
                .filter(|_| !matches!(s.popup(), Some(Popup::ConfirmQuit { .. })));
            match busy {
                Some(busy) => Dispatcher::show_popup(Popup::ConfirmQuit { busy }, cx),
                None => cx.quit(),
            }
        });
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &InstallCli, cx| Dispatcher::install_cli(cx));
        on_menu_action(cx, |_: &ImportFromGitHubDesktop, cx| {
            let enabled = corvene_core::AppState::global(cx)
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::IMPORT_FROM_GITHUB_DESKTOP);
            if enabled {
                Dispatcher::show_popup(Popup::ImportFromGitHubDesktop, cx)
            }
        });
        on_menu_action(cx, |_: &RemoveRepositories, cx| {
            let s = corvene_core::AppState::global(cx).read(cx);
            if s.flags
                .bool(corvene_core::flags::ids::BULK_REMOVE_REPOSITORIES)
                && !s.repositories.is_empty()
            {
                Dispatcher::show_popup(Popup::RemoveRepositories { ticked: None }, cx)
            }
        });
        on_menu_action(cx, |_: &AddLocalRepository, cx| {
            Dispatcher::show_popup(Popup::AddExistingRepository { path: None }, cx)
        });
        on_menu_action(cx, |_: &NewRepository, cx| {
            Dispatcher::show_popup(Popup::CreateRepository { path: None }, cx)
        });
        on_menu_action(cx, |_: &CloneRepository, cx| {
            Dispatcher::show_popup(Popup::CloneRepository { url: None }, cx)
        });
        // CORVENE_DEV_ACCOUNTS="login@api-base,…" adds token-less accounts to
        // this session only (dev/testing convenience for the account pickers).
        if let Ok(spec) = std::env::var("CORVENE_DEV_ACCOUNTS") {
            corvene_core::AppState::global(cx).update(cx, |s, cx| {
                for (ix, item) in spec.split(',').enumerate() {
                    if let Some((login, endpoint)) = item.split_once('@') {
                        s.accounts.push(corvene_core::Account {
                            endpoint: endpoint.to_string(),
                            id: 1_000_000 + ix as u64,
                            login: login.to_string(),
                            name: None,
                            avatar_url: None,
                            emails: Vec::new(),
                            scopes: Vec::new(),
                            plan: None,
                            private_primary_email: false,
                            from_gh_cli: false,
                        });
                    }
                }
                cx.notify();
            });
        }
        // CORVENE_SNAPSHOT=<png> (build with `--features snapshots`): render the
        // window offscreen after CORVENE_SNAPSHOT_DELAY_MS (default 4000), save
        // it and quit - visual checks without screen-recording permission.
        #[cfg(feature = "snapshots")]
        if let Ok(path) = std::env::var("CORVENE_SNAPSHOT") {
            let delay = std::env::var("CORVENE_SNAPSHOT_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4000u64);
            cx.spawn(async move |cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(delay))
                    .await;
                // an unfocused window only renders when drawn: draw once so
                // views start their background work (diff highlighting), give
                // it time, then draw the frame that is saved
                cx.update(|cx| {
                    for handle in cx.windows() {
                        let _ = handle.update(cx, |_, window, cx| {
                            window.refresh();
                            window.draw(cx).clear(cx);
                        });
                    }
                });
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                cx.update(|cx| {
                    // `429-multiple-windows`: the first window at `path`,
                    // the others at `<stem>-2.png`, `<stem>-3.png`
                    for (index, handle) in cx.windows().into_iter().enumerate() {
                        let path = if index == 0 {
                            path.clone()
                        } else {
                            let p = std::path::Path::new(&path);
                            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("snapshot");
                            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("png");
                            p.with_file_name(format!("{stem}-{}.{ext}", index + 1))
                                .to_string_lossy()
                                .into_owned()
                        };
                        let image = handle.update(cx, |_, window, cx| {
                            // draw now: an unfocused window gets no display-link
                            // frames, and render_to_image uses the last scene
                            window.refresh();
                            window.draw(cx).clear(cx);
                            window.render_to_image()
                        });
                        match image {
                            Ok(Ok(image)) => match image.save(&path) {
                                Ok(()) => info!(
                                    path,
                                    zoom = corvene_ui::theme::sizes::zoom_factor(),
                                    "snapshot saved"
                                ),
                                Err(err) => error!(?err, "could not save the snapshot"),
                            },
                            Ok(Err(err)) => error!(?err, "could not render the snapshot"),
                            Err(err) => error!(?err, "no window for the snapshot"),
                        }
                    }
                    cx.quit();
                });
            })
            .detach();
        }
        // CORVENE_CONTROL=<port> (build with `--features snapshots`): remote
        // control for the GitHub Desktop parity harness (`tools/parity`).
        #[cfg(feature = "snapshots")]
        if let Some(port) = std::env::var("CORVENE_CONTROL")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            parity_control::start(port, open_dev_popup, cx);
        }
        // CORVENE_ADD_REPO=/path adds a repository at launch (dev/testing convenience).
        if let Ok(path) = std::env::var("CORVENE_ADD_REPO") {
            Dispatcher::add_repository(std::path::PathBuf::from(path), cx);
        }
        // `--open-repo <path>` (GHD `--cli-open`; the command line tool now
        // sends an `openLocalRepo` URL instead): select the repository, or
        // offer to add it.
        if let Some(path) = open_repo_argument(std::env::args()) {
            Dispatcher::open_local_repository(path, cx);
        }
        // CORVENE_CLONE="<url>|<path>" clones at launch (dev/testing convenience).
        if let Ok(spec) = std::env::var("CORVENE_CLONE")
            && let Some((url, path)) = spec.split_once('|')
        {
            Dispatcher::clone_repository(url.to_string(), std::path::PathBuf::from(path), None, cx);
        }
        // CORVENE_POPUP=<name> opens a dialog at launch (dev/testing convenience
        // for headless smoke runs; API-backed dialogs get sample data):
        //   preferences[:<tab>] (accounts, integrations, git, appearance, notifications,
        //   prompts, advanced, accessibility) | repository-settings | about | create
        //   clone | clone:<url>
        //   release-notes | move-to-applications | upstream-already-exists
        //   push-needs-pull | initialize-lfs | oversized-files (for the selected repository)
        //   pr-review[:approved|:commented] (changes requested by default)
        //   pr-comment | pr-checks-failed
        //   job-log (the sample failed job's Actions log, flag 347)
        //   pr-list (sample pull requests in the branch foldout's Pull Requests tab)
        //   pull-request-review (the review of a sample pull request with sample threads;
        //     `348-pull-request-review`)
        //   tutorial:<step> (the repository becomes the tutorial repository, shown at
        //   <step>: pick-editor, create-branch, edit-file, make-commit, push-branch,
        //   open-pull-request, all-done, announced, paused)
        //   tutorial-create | tutorial-exit (the two tutorial dialogs; tutorial-create
        //   needs CORVENE_DEV_ACCOUNTS)
        //   crash-report (turns on "Save crash reports locally" and panics, so the
        //   next launch shows "Corvene quit unexpectedly last time")
        //   no-write-access (the repository becomes a read-only GitHub repository;
        //   add CORVENE_DEV_ACCOUNTS=login@https://api.github.com for the fork dialog)
        //   test-notifications (Help › Show Test Notifications: posts sample
        //   review / comment / checks-failed notifications; needs the .app bundle)
        //   notification-click:review|comment|checks-failed (what clicking such a
        //   notification does: its userInfo payload goes through the click handler)
        //   zoom-in | zoom-out | zoom-reset (View › Zoom, with the zoom overlay)
        //   sign-in (the GitHub.com sign-in dialog)
        //   alive:review|comment|checks-failed[:api] (an Alive event for the sample
        //   pull requests through the notification handler; sample data unless :api)
        //   pr-event:review-requested|reviewed|merged|mentioned (a sample pull request
        //   event notification for the selected repository, flag 354)
        //   update-available[:brew|:pkg][:about|:notes] (a sample update in the
        //   ready / Homebrew / package manager state: the banner, plus About or
        //   the Release Notes with "Install and Restart")
        //   flags[:<search>] (Corvene › Flags…, with the search box prefilled)
        //   remove-repositories (File › Remove Repositories…, the selected one ticked)
        //   repository-list (the repository foldout)
        //   language-extensions[:find|:find=<suffix>|:import|:icons|:consent] (Settings › Appearance ›
        //   Language extensions…; flag 111; :consent offers the first grammar waiting for a
        //   build from source, flag 1001)
        //   git-error[:raw|:known|:push|:plain] (the error dialog for a failed pull:
        //   a merge blocked by local changes, an output nobody has words for, a
        //   failure GHD describes, a push a protected branch rejected, or an error
        //   git did not produce)
        //   recent-activity (Repository › Recent Activity…, flag 1216),
        //   insights[:all] (Repository › Insights…, :all over all time; flag 1110),
        //   compare-refs[:<base>[:<head>[:...]]] (Branch › Compare…, or with
        //   both refs the comparison itself, `...` for base...head; flag 1218)
        //   tags (Branch › Tags…, flag 1219),
        //   create-ssh-key | ssh-key-needs-scope (Settings › Integrations' SSH
        //   key dialogs, flag 350),
        //   issues | new-issue (Repository › Issues… with sample issues, New
        //   Issue…; flag 345), releases | create-release[:<tag>] (Repository ›
        //   Releases… with sample releases, Create Release…; flag 346)
        //   actions | run-workflow (Repository › Actions… with sample workflows,
        //   runs and jobs, and Run workflow's form; flag 351)
        //   blame:<path>[@<rev>] (the Blame view of a file, in the working tree
        //   or at a revision; flag 798)
        //   clean-untracked[:ignored] (Repository › Clean Untracked Files…, flag 1105)
        //   apply-patch:<path> (Apply Patch's preview of a patch file, the path
        //   relative to the repository; flag 1106)
        //   submodules (Repository › Submodules…, flag 1111)
        //   sparse-checkout (Repository › Sparse Checkout…, flag 1112)
        //   force-unlock:<path> (Force Unlock of an LFS lock, flag 1113)
        //   new-changelist[:<path>[,<path>…]] (New Changelist… for those files, else
        //   the selected ones; flag 1313), changelists:<Name>=<path>[,<path>…][;…]
        //   (creates those changelists outright)
        if let Ok(popup) = std::env::var("CORVENE_POPUP") {
            // Deferred so a `CORVENE_ADD_REPO` repository has been added and refreshed.
            cx.spawn(async move |cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                cx.update(|cx| {
                    open_dev_popup(&popup, cx);
                });
            })
            .detach();
        }
        on_menu_action(cx, |_: &RemoveRepository, cx| {
            if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                Dispatcher::request_remove_repository(id, cx);
            }
        });
        on_menu_action(cx, |_: &MoveToSharedStorage, cx| {
            if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                Dispatcher::show_popup(
                    Popup::MoveToSharedStorage {
                        repo: id,
                        then: corvene_core::AfterSharedStorageMove::Nothing,
                    },
                    cx,
                );
            }
        });
        on_menu_action(cx, |_: &OpenFlags, cx| Dispatcher::open_flags(None, cx));
        // Corvene (`610-diff-mode-shortcut`): Diff Settings › Unified / Split
        on_menu_action(cx, |_: &ToggleDiffDisplayMode, cx| {
            let split = corvene_core::AppState::global(cx)
                .read(cx)
                .settings
                .show_side_by_side_diff;
            Dispatcher::set_show_side_by_side_diff(!split, cx);
        });
        // Corvene (`1304-diff-no-wrap`): View › Wrap Diff Lines
        on_menu_action(cx, |_: &ToggleDiffWordWrap, cx| {
            let wrap = corvene_core::AppState::global(cx)
                .read(cx)
                .settings
                .diff_wrap_lines;
            Dispatcher::set_diff_wrap_lines(!wrap, cx);
        });
        // Corvene (`1310-file-list-tree`): View › Show Changes as Tree / as List
        // `1117-browse-files`
        on_menu_action(cx, |_: &BrowseFiles, cx| {
            if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                Dispatcher::show_browse_files(id, None, cx);
            }
        });
        // `431-repository-sidebar`
        on_menu_action(cx, |_: &ToggleRepositorySidebar, cx| {
            Dispatcher::update_settings(cx, |s| s.repository_sidebar = !s.repository_sidebar);
        });
        // `432-history-file-list`
        on_menu_action(cx, |_: &HistoryFileListBeside, cx| {
            Dispatcher::update_settings(cx, |s| {
                s.history_file_list = corvene_core::HistoryFileList::Beside
            });
        });
        on_menu_action(cx, |_: &HistoryFileListAbove, cx| {
            Dispatcher::update_settings(cx, |s| {
                s.history_file_list = corvene_core::HistoryFileList::Above
            });
        });
        on_menu_action(cx, |_: &HistoryFileListHidden, cx| {
            Dispatcher::update_settings(cx, |s| {
                s.history_file_list = corvene_core::HistoryFileList::Hidden
            });
        });
        on_menu_action(cx, |_: &ToggleFileListTree, cx| {
            let tree = corvene_core::AppState::global(cx)
                .read(cx)
                .settings
                .file_list_tree;
            Dispatcher::set_file_list_tree(!tree, cx);
        });
        // Corvene (`1311-stacked-diff`): the tab's stack button, by action
        on_menu_action(cx, |_: &ToggleStackedDiff, cx| {
            let toggle = {
                let s = corvene_core::AppState::global(cx).read(cx);
                corvene_core::stacked_diff::enabled(s).then(|| {
                    let history = s
                        .selected_state()
                        .is_some_and(|rs| rs.section == Section::History);
                    let on = if history {
                        s.settings.stacked_diff_history
                    } else {
                        s.settings.stacked_diff_changes
                    };
                    (history, on)
                })
            };
            if let Some((history, on)) = toggle {
                Dispatcher::set_stacked_diff(history, !on, cx);
            }
        });
        on_menu_action(cx, |_: &OpenSettings, cx| {
            Dispatcher::open_preferences(corvene_core::PreferencesTab::Accounts, cx)
        });
        on_menu_action(cx, |_: &About, cx| {
            Dispatcher::show_popup(
                Popup::About {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
                cx,
            )
        });
        let selected_path = |cx: &App| -> Option<(u64, std::path::PathBuf)> {
            let s = corvene_core::AppState::global(cx).read(cx);
            let repo = s.selected_repository()?;
            Some((repo.id, repo.path.clone()))
        };
        on_menu_action(cx, move |_: &AddLicense, cx| {
            let enabled = corvene_core::AppState::global(cx)
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::ADD_LICENSE);
            if enabled && let Some((id, _)) = selected_path(cx) {
                Dispatcher::show_popup(Popup::AddLicense { repo: id }, cx);
            }
        });
        // `423-undo-commit-menu-item`: the Undo bar's button
        on_menu_action(cx, move |_: &UndoLastCommit, cx| {
            let s = corvene_core::AppState::global(cx).read(cx);
            let id = s.selected.filter(|_| {
                s.flags
                    .bool(corvene_core::flags::ids::UNDO_COMMIT_MENU_ITEM)
                    && s.selected_state()
                        .is_some_and(|rs| rs.last_commit.is_some())
            });
            if let Some(id) = id {
                Dispatcher::request_undo_last_commit(id, cx);
            }
        });
        on_menu_action(cx, move |_: &RepositorySettings, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::open_repository_settings(
                    id,
                    corvene_core::RepositorySettingsTab::Remote,
                    cx,
                );
            }
        });
        on_kept_menu_action(
            cx,
            MenuId::OpenExternalEditor,
            move |_: &OpenInEditor, cx| {
                if let Some((_, path)) = selected_path(cx) {
                    Dispatcher::open_in_editor(path, cx);
                }
            },
        );
        on_kept_menu_action(cx, MenuId::OpenInShell, move |_: &OpenInShell, cx| {
            if let Some((_, path)) = selected_path(cx) {
                Dispatcher::open_in_shell(&path, cx);
            }
        });
        on_kept_menu_action(
            cx,
            MenuId::OpenWorkingDirectory,
            move |_: &ShowInFinder, cx| {
                if let Some((_, path)) = selected_path(cx) {
                    Dispatcher::show_repository(&path, cx);
                }
            },
        );
        on_menu_action(cx, move |_: &OpenWith, cx| {
            if let Some((_, path)) = selected_path(cx) {
                Dispatcher::open_with(path, cx);
            }
        });
        on_kept_menu_action(
            cx,
            MenuId::ViewRepositoryOnGithub,
            move |_: &ViewOnGitHub, cx| {
                if let Some((id, _)) = selected_path(cx) {
                    Dispatcher::view_on_github(id, cx);
                }
            },
        );
        on_menu_action(cx, move |_: &ViewUpstreamOnGitHub, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::view_upstream_on_github(id, cx);
            }
        });
        on_menu_action(cx, move |_: &CreateIssue, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::create_issue(id, cx);
            }
        });
        on_menu_action(cx, move |_: &CompareOnGitHub, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::compare_on_github(id, cx);
            }
        });
        on_menu_action(cx, move |_: &ViewBranchOnGitHub, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::view_branch_on_github(id, cx);
            }
        });
        on_menu_action(cx, move |_: &CreatePullRequest, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::create_pull_request(id, cx);
            }
        });
        on_menu_action(cx, move |_: &PreviewPullRequest, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::start_pull_request(id, cx);
            }
        });
        on_menu_action(cx, move |_: &MoveChangesToWorktree, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::show_move_changes_to_worktree(id, cx);
            }
        });
        on_menu_action(cx, move |_: &StashAllChangesWithMessage, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::request_stash_with_message(id, cx);
            }
        });
        on_menu_action(cx, move |_: &RequestReviewers, cx| {
            if let Some((id, _)) = selected_path(cx) {
                Dispatcher::show_request_reviewers(id, cx);
            }
        });
        // Help
        cx.on_action(|_: &ReportIssue, cx| {
            Dispatcher::open_url("https://github.com/wasi-master/corvene/issues/new", cx)
        });
        cx.on_action(|_: &ContactSupport, cx| {
            Dispatcher::open_url("https://github.com/wasi-master/corvene/discussions", cx)
        });
        cx.on_action(|_: &ShowReleaseNotes, cx| Dispatcher::show_release_notes(cx));
        // GHD test menu "Show notification" (`testShowNotification`)
        #[cfg(debug_assertions)]
        cx.on_action(|_: &ShowTestNotifications, cx| {
            if let Some(id) = corvene_core::AppState::global(cx).read(cx).selected {
                Dispatcher::show_popup(Popup::TestNotifications { repo: id }, cx);
            }
        });
        cx.on_action(|_: &ShowUserGuides, cx| {
            Dispatcher::open_url("https://docs.github.com/en/desktop", cx)
        });
        cx.on_action(|_: &ShowKeyboardShortcuts, cx| {
            Dispatcher::open_url(
                "https://docs.github.com/en/desktop/overview/github-desktop-keyboard-shortcuts",
                cx,
            )
        });
        cx.on_action(|_: &ShowLogs, cx| {
            let dir = corvene_platform::paths::logs_dir();
            let _ = std::fs::create_dir_all(&dir);
            Dispatcher::show_in_finder(&dir, cx);
        });
        // Window
        cx.on_action(|_: &CloseWindow, cx| {
            // `430-repository-tabs`: ⌘W closes the tab while there is another
            let tabs = corvene_core::AppState::global(cx)
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::REPOSITORY_TABS);
            if tabs && Dispatcher::close_tab(None, cx) {
                return;
            }
            // `429-multiple-windows`: a window that is not the last closes
            // for good (`on_window_closed` drops its workspace)
            if corvene_ui::windows::count(cx) > 1 {
                defer_in_active_window(cx, |window, _| window.remove_window());
                return;
            }
            // GHD hides the window and keeps running; the Dock brings it back.
            #[cfg(target_os = "macos")]
            defer_in_active_window(cx, |window, cx| {
                corvene_ui::native_window::hide_window(window, cx)
            });
            #[cfg(not(target_os = "macos"))]
            cx.hide();
        });
        cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
        cx.on_action(|_: &ShowMainWindow, cx| focus_main_window(cx));
        // `429-multiple-windows`
        on_menu_action(cx, |_: &NewWindow, cx| corvene_ui::windows::new_window(cx));
        // `430-repository-tabs`
        on_menu_action(cx, |_: &NextTab, cx| Dispatcher::step_tab(true, cx));
        on_menu_action(cx, |_: &PreviousTab, cx| Dispatcher::step_tab(false, cx));
        on_menu_action(cx, |_: &CloseTab, cx| {
            Dispatcher::close_tab(None, cx);
        });

        // Chromium's text antialiasing follows the desktop's (grayscale
        // unless it asks for subpixel order); GPUI would use subpixel
        // wherever the GPU can
        #[cfg(not(target_os = "macos"))]
        if !corvene_platform::fonts::subpixel_antialiasing() {
            cx.set_text_rendering_mode(TextRenderingMode::Grayscale);
        }
        // Linux: Electron's classic menu bar over the app (`corvene_ui::menu_bar`)
        #[cfg(not(target_os = "macos"))]
        corvene_ui::views_menu::install(cx);
        // `429-multiple-windows`: one window per workspace (one in GitHub
        // Desktop's model), opened through `corvene_ui::windows` so "Open in
        // New Window" and File › New Window can open more later
        // (`open_workspace_window`); a closed window drops its workspace.
        {
            let state = state.clone();
            corvene_ui::windows::set_opener(
                move |workspace, cx| {
                    open_workspace_window(workspace, state.clone(), sidebar_width, started, cx)
                },
                cx,
            );
        }
        cx.on_window_closed(|cx, window| {
            if let Some(workspace) = corvene_ui::windows::unregister(window, cx) {
                Dispatcher::close_workspace(workspace, cx);
            }
        })
        .detach();
        let workspaces: Vec<corvene_core::WorkspaceId> =
            state.read(cx).workspaces.iter().map(|w| w.id).collect();
        for workspace in workspaces {
            open_workspace_window(workspace, state.clone(), sidebar_width, started, cx);
        }
        if corvene_ui::windows::count(cx) == 0 {
            error!("failed to open main window");
            cx.quit();
            return;
        }

        // Once the window is on screen: AppKit takes 30-60 ms to build the
        // macOS menu bar (the Edit menu's text input items load the Writing
        // Tools library), and the crash report check records this launch
        // with a durable settings write. The window reaches the screen when
        // the run loop next sleeps, which the first frame callback can still
        // run before, so the second one does it. A window that draws no
        // frames (shown later, occluded) gets them from the timer.
        let after_first_frame =
            std::rc::Rc::new(std::cell::Cell::new(Some(move |cx: &mut App| {
                #[cfg(target_os = "macos")]
                {
                    let options =
                        menus::MenuOptions::of(corvene_core::AppState::global(cx).read(cx));
                    menus::install(cx, &options);
                }
                Dispatcher::check_crash_reports(cx);
                phase(started, "menus installed, crash reports checked");
            })));
        if let Some(window) = cx.windows().first().copied() {
            let run = after_first_frame.clone();
            window
                .update(cx, |_, window, _| {
                    window.on_next_frame(move |window, _| {
                        window.on_next_frame(move |_, cx| {
                            if let Some(f) = run.take() {
                                f(cx)
                            }
                        })
                    })
                })
                .ok();
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            if let Some(f) = after_first_frame.take() {
                cx.update(f);
            }
        })
        .detach();

        // View / Window actions are global so the menu items stay enabled whatever has focus.
        on_menu_action(cx, move |_: &ShowChanges, cx| {
            with_focused_workspace(cx, |w, cx| w.switch_section(Section::Changes, cx))
        });
        on_menu_action(cx, move |_: &ShowHistory, cx| {
            with_focused_workspace(cx, |w, cx| w.switch_section(Section::History, cx))
        });
        on_menu_action(cx, move |_: &ToggleSection, cx| {
            with_focused_workspace(cx, |w, cx| {
                let next = match w.section() {
                    Section::Changes => Section::History,
                    Section::History => Section::Changes,
                };
                w.switch_section(next, cx)
            })
        });
        on_menu_action(cx, move |_: &ShowRepositoryList, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.show_repository_list(window, cx));
        });
        on_menu_action(cx, move |_: &ShowBranchesList, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.show_branches_list(window, cx));
        });
        on_menu_action(cx, move |_: &ShowWorktreesList, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.show_worktrees_list(window, cx));
        });
        // Corvene (`427-back-forward-navigation`)
        on_menu_action(cx, |_: &NavigateBack, cx| Dispatcher::navigate(true, cx));
        on_menu_action(cx, |_: &NavigateForward, cx| {
            Dispatcher::navigate(false, cx)
        });
        // Corvene (`801-history-review-mode`)
        on_menu_action(cx, move |_: &ToggleHistoryReviewMode, cx| {
            with_focused_workspace(cx, |w, cx| w.toggle_review_mode(cx))
        });
        // Corvene (`612-navigation-shortcuts`)
        on_menu_action(cx, move |_: &ShowPullRequestsList, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.show_pull_requests_list(window, cx));
        });
        on_menu_action(cx, move |_: &FocusDiff, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.focus_diff(window, cx));
        });
        cx.on_action(move |_: &SelectNextFileFromDiff, cx| {
            with_focused_workspace(cx, |w, cx| w.step_file(1, cx))
        });
        cx.on_action(move |_: &SelectPreviousFileFromDiff, cx| {
            with_focused_workspace(cx, |w, cx| w.step_file(-1, cx))
        });
        let step_repository = |step: isize, cx: &mut App| {
            let next = {
                let s = corvene_core::AppState::global(cx).read(cx);
                corvene_ui::repository_list::step_repository(
                    &corvene_ui::repository_list::list_order(s),
                    s.selected,
                    step,
                )
            };
            if let Some(id) = next {
                Dispatcher::select_repository(id, cx);
            }
        };
        on_menu_action(cx, move |_: &NextRepository, cx| step_repository(1, cx));
        on_menu_action(cx, move |_: &PreviousRepository, cx| {
            step_repository(-1, cx)
        });
        on_menu_action(cx, move |_: &GoToSummary, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.focus_commit_summary(window, cx));
        });
        cx.on_action(move |_: &Find, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.focus_filter(window, cx));
        });
        // `624-command-palette`
        on_menu_action(cx, |_: &CommandPalette, cx| {
            defer_in_focused_workspace(cx, |_, window, cx| {
                corvene_ui::dialogs::command_palette::open(window, cx)
            });
        });
        on_menu_action(cx, move |_: &ToggleChangesFilter, cx| {
            with_focused_workspace(cx, |w, cx| w.toggle_changes_filter(cx))
        });
        // View › Reset Zoom / Zoom In / Zoom Out (GHD `zoom(ZoomDirection)`)
        cx.on_action(move |_: &ZoomIn, cx| with_focused_workspace(cx, |w, cx| w.zoom(1, cx)));
        cx.on_action(move |_: &ZoomOut, cx| with_focused_workspace(cx, |w, cx| w.zoom(-1, cx)));
        cx.on_action(move |_: &ResetZoom, cx| with_focused_workspace(cx, |w, cx| w.zoom(0, cx)));
        on_menu_action(cx, move |_: &CompareToBranch, cx| {
            defer_in_focused_workspace(cx, |w, window, cx| w.show_compare(window, cx));
        });
        // Branch menu
        let selected = |cx: &App| corvene_core::AppState::global(cx).read(cx).selected;
        on_menu_action(cx, move |_: &NewBranch, cx| {
            if let Some(id) = selected(cx) {
                // `847-new-branch-from-filter`: like the foldout's New Branch
                // button, start from the branch filter's text
                let from_filter = corvene_core::AppState::global(cx)
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::NEW_BRANCH_FROM_FILTER)
                    .then(|| {
                        corvene_ui::windows::focused(cx)
                            .and_then(|e| e.view.read(cx).open_branch_filter(cx))
                    })
                    .flatten();
                if from_filter.is_some() {
                    Dispatcher::close_foldout(cx);
                }
                Dispatcher::show_popup(
                    Popup::CreateBranch {
                        repo: id,
                        target_sha: None,
                        initial_name: from_filter.unwrap_or_default(),
                    },
                    cx,
                );
            }
        });
        on_menu_action(cx, move |_: &NewWorktree, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_popup(
                    Popup::AddWorktree {
                        repo: id,
                        initial_branch_name: None,
                        initial_worktree_name: None,
                    },
                    cx,
                );
            }
        });
        let current_branch = |cx: &App| -> Option<(u64, String)> {
            let s = corvene_core::AppState::global(cx).read(cx);
            let id = s.selected?;
            let name = s
                .repo_states
                .get(&id)?
                .info
                .as_ref()?
                .current_branch()?
                .name
                .clone();
            Some((id, name))
        };
        on_menu_action(cx, move |_: &RenameBranch, cx| {
            if let Some((id, name)) = current_branch(cx) {
                Dispatcher::show_popup(Popup::RenameBranch { repo: id, name }, cx);
            }
        });
        on_menu_action(cx, move |_: &DeleteBranch, cx| {
            if let Some((id, name)) = current_branch(cx) {
                Dispatcher::show_popup(Popup::DeleteBranch { repo: id, name }, cx);
            }
        });
        on_menu_action(cx, move |_: &MergeIntoCurrentBranch, cx| {
            if let Some((id, _)) = current_branch(cx)
                && !Dispatcher::refuse_merge_while_conflicted(id, cx)
            {
                Dispatcher::show_popup(
                    Popup::MergeBranch {
                        repo: id,
                        squash: false,
                    },
                    cx,
                );
            }
        });
        on_menu_action(cx, move |_: &SquashAndMergeIntoCurrentBranch, cx| {
            if let Some((id, _)) = current_branch(cx)
                && !Dispatcher::refuse_merge_while_conflicted(id, cx)
            {
                Dispatcher::show_popup(
                    Popup::MergeBranch {
                        repo: id,
                        squash: true,
                    },
                    cx,
                );
            }
        });
        // GHD's push item emits `force-push` (and reads Force Push) whenever
        // a force push is possible (`build-default-menu.ts`, `app.tsx#push`)
        on_menu_action(cx, move |_: &Push, cx| {
            if let Some(id) = selected(cx) {
                if Dispatcher::force_push_state(id, cx)
                    != corvene_core::ForcePushState::NotAvailable
                {
                    Dispatcher::confirm_or_force_push(id, cx);
                } else {
                    Dispatcher::push(id, false, None, cx);
                }
            }
        });
        on_menu_action(cx, move |_: &Pull, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::pull(id, cx);
            }
        });
        on_menu_action(cx, move |_: &Fetch, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::fetch(id, false, cx);
            }
        });
        on_menu_action(cx, move |_: &FetchAllRepositories, cx| {
            Dispatcher::fetch_all_repositories(cx);
        });
        on_menu_action(cx, move |_: &PullAllRepositories, cx| {
            Dispatcher::pull_all_repositories(cx);
        });
        // `1210-push-to-other-remote`: the remote at the item's index of the
        // menu's list (`Dispatcher::menu_remotes`, which built the menu)
        fn menu_remote(cx: &App, index: usize) -> Option<(u64, String)> {
            let s = corvene_core::AppState::global(cx).read(cx);
            let id = s.selected?;
            Some((id, Dispatcher::menu_remotes(s, id).into_iter().nth(index)?))
        }
        macro_rules! remote_menu_actions {
            ($($index:literal => $push:ident, $fetch:ident;)*) => {$(
                on_menu_action(cx, move |_: &$push, cx| {
                    if let Some((id, remote)) = menu_remote(cx, $index) {
                        Dispatcher::push_to_remote(id, remote, cx);
                    }
                });
                on_menu_action(cx, move |_: &$fetch, cx| {
                    if let Some((id, remote)) = menu_remote(cx, $index) {
                        Dispatcher::fetch_from_remote(id, remote, cx);
                    }
                });
            )*};
        }
        remote_menu_actions! {
            0 => PushToRemote0, FetchFromRemote0;
            1 => PushToRemote1, FetchFromRemote1;
            2 => PushToRemote2, FetchFromRemote2;
            3 => PushToRemote3, FetchFromRemote3;
            4 => PushToRemote4, FetchFromRemote4;
            5 => PushToRemote5, FetchFromRemote5;
            6 => PushToRemote6, FetchFromRemote6;
            7 => PushToRemote7, FetchFromRemote7;
        }
        // `524-open-repository-with-editor`: the editor at the item's index
        // of the menu's list (`Dispatcher::menu_editors`)
        fn open_in_chosen_editor(cx: &mut App, index: usize) {
            let target = {
                let s = corvene_core::AppState::global(cx).read(cx);
                s.selected_repository()
                    .filter(|r| !r.missing)
                    .map(|r| r.path.clone())
                    .zip(Dispatcher::menu_editors(s).into_iter().nth(index))
            };
            if let Some((path, (_, editor))) = target {
                Dispatcher::open_in_editor_with(path, editor, cx);
            }
        }
        macro_rules! editor_menu_actions {
            ($($index:literal => $action:ident;)*) => {$(
                on_menu_action(cx, move |_: &$action, cx| open_in_chosen_editor(cx, $index));
            )*};
        }
        editor_menu_actions! {
            0 => OpenInChosenEditor0;
            1 => OpenInChosenEditor1;
            2 => OpenInChosenEditor2;
            3 => OpenInChosenEditor3;
            4 => OpenInChosenEditor4;
            5 => OpenInChosenEditor5;
            6 => OpenInChosenEditor6;
            7 => OpenInChosenEditor7;
        }
        on_menu_action(cx, move |_: &FetchAllTags, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::fetch_all_tags(id, cx);
            }
        });
        // `1216-recent-activity`
        // `1219-tag-manager`
        on_menu_action(cx, move |_: &ShowTags, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_tags(id, cx);
            }
        });
        // `1223-checkout-from-fork`
        on_menu_action(cx, move |_: &CheckoutFromFork, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_checkout_from_fork(id, None, None, cx);
            }
        });
        on_menu_action(cx, move |_: &ShowRecentActivity, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_recent_activity(id, cx);
            }
        });
        // `1110-repository-insights`
        on_menu_action(cx, move |_: &ShowInsights, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_insights(id, cx);
            }
        });
        // `1218-compare-refs`: the current branch as the second ref
        on_menu_action(cx, move |_: &CompareRefs, cx| {
            if let Some(id) = selected(cx) {
                let head = corvene_ui::history::current_ref(id, cx);
                Dispatcher::show_compare_refs(id, None, head, cx);
            }
        });
        // `356-stacked-pull-requests`
        on_menu_action(cx, move |_: &CreateStackedPullRequests, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_create_stacked_pull_requests(id, cx);
            }
        });
        on_menu_action(cx, move |_: &PushStackedBranches, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::push_stacked_branches(id, cx);
            }
        });
        // `1105-clean-untracked-files`
        on_menu_action(cx, move |_: &CleanUntrackedFiles, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_clean_untracked_files(id, cx);
            }
        });
        // `1111-submodules`
        on_menu_action(cx, move |_: &ShowSubmodules, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_submodules(id, cx);
            }
        });
        // `1112-sparse-checkout`
        on_menu_action(cx, move |_: &ShowSparseCheckout, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_sparse_checkout(id, cx);
            }
        });
        // `1106-apply-patch`
        on_menu_action(cx, move |_: &ApplyPatchFromFile, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::prompt_apply_patch_file(id, cx);
            }
        });
        on_menu_action(cx, move |_: &ApplyPatchFromClipboard, cx| {
            if let Some(id) = selected(cx) {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                Dispatcher::preview_patch_text(id, text, cx);
            }
        });
        // `348-pull-request-review`
        on_menu_action(cx, move |_: &ReviewPullRequest, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::review_current_pull_request(id, cx);
            }
        });
        // `345-issues`
        on_menu_action(cx, move |_: &ShowIssues, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_issues(id, cx);
            }
        });
        on_menu_action(cx, move |_: &NewIssue, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_popup(Popup::NewIssue { repo: id }, cx);
            }
        });
        // `346-releases`
        on_menu_action(cx, move |_: &ShowReleases, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_releases(id, cx);
            }
        });
        on_menu_action(cx, move |_: &CreateRelease, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_create_release(id, None, None, cx);
            }
        });
        // `351-actions`
        on_menu_action(cx, move |_: &ShowActions, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::show_repository_actions(id, cx);
            }
        });
        // `1212-bisect`
        on_menu_action(cx, move |_: &StartBisect, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::request_start_bisect(id, None, cx);
            }
        });
        on_menu_action(cx, move |_: &StopBisect, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::stop_bisect(id, cx);
            }
        });
        Dispatcher::start_background_tasks(cx);
        // `296-cancel-fetch-on-wake`: a background fetch that hangs on a
        // connection the sleep broke is stopped
        cx.on_system_wake(|cx| Dispatcher::system_woke(cx)).detach();
        Dispatcher::refresh_accounts(cx);
        Dispatcher::sync_gh_cli_account(cx);
        // Alive subscriptions for pull request notifications (GHD AliveStore)
        Dispatcher::start_alive(cx);
        // `354-pull-request-event-notifications` (idle while the flag is off)
        Dispatcher::start_pull_request_event_polling(cx);
        Dispatcher::start_pull_request_updater(cx);
        Dispatcher::start_commit_status_refresh(cx);
        // flags 342-344: OAuth tokens of GitLab and Gitea accounts expire
        Dispatcher::start_host_token_refresh(cx);
        // GHD `componentDidMount`: offer the move to /Applications
        Dispatcher::check_move_to_applications_folder(cx);
        // `checkForUpdates(true)` at launch and every four hours (release builds)
        Dispatcher::start_update_checks(cx);
        // on-demand packs installed earlier (extended grammars)
        Dispatcher::load_installed_packs(cx);
        Dispatcher::load_language_extensions(cx);
        on_menu_action(cx, move |_: &RebaseCurrentBranch, cx| {
            if let Some((id, _)) = current_branch(cx)
                && !Dispatcher::refuse_merge_while_conflicted(id, cx)
            {
                Dispatcher::start_rebase_flow(id, cx);
            }
        });
        on_menu_action(cx, move |_: &UpdateFromDefaultBranch, cx| {
            if let Some((id, _)) = current_branch(cx)
                && !Dispatcher::refuse_merge_while_conflicted(id, cx)
            {
                Dispatcher::update_from_default_branch(id, cx);
            }
        });
        on_menu_action(cx, move |_: &StashAllChanges, cx| {
            if let Some((id, _)) = current_branch(cx) {
                Dispatcher::stash_all_changes(id, cx);
            }
        });
        on_menu_action(cx, move |_: &ToggleStashedChanges, cx| {
            if let Some(id) = selected(cx) {
                Dispatcher::toggle_stash_view(id, cx);
            }
        });
        on_menu_action(cx, move |_: &DiscardAllChanges, cx| {
            if let Some(id) = selected(cx) {
                let paths: Vec<String> = corvene_core::AppState::global(cx)
                    .read(cx)
                    .repo_states
                    .get(&id)
                    .and_then(|r| r.status.as_deref())
                    .map(|st| st.files.iter().map(|f| f.path.clone()).collect())
                    .unwrap_or_default();
                Dispatcher::request_discard_changes(id, paths, cx);
            }
        });
        cx.on_action(|_: &CloseFoldout, cx| {
            let state = corvene_core::AppState::global(cx).read(cx);
            let open = state.foldout;
            if open.is_none() && state.popup().is_none() {
                // nothing to close: Escape goes on to the focused element's
                // binding (History's compare box, a commit reorder), which
                // this context-free one outranks
                cx.propagate();
                return;
            }
            // the toolbar button that opened the foldout keeps keyboard focus
            let ring = open.is_some() && corvene_ui::toolbar::opened_by_button(cx) == open;
            Dispatcher::close_foldout(cx);
            Dispatcher::close_popup(cx);
            if ring {
                corvene_ui::toolbar::set_focus_visible(open, cx);
            }
        });
        cx.on_action(|_: &Minimize, cx| {
            defer_in_active_window(cx, |window, _| window.minimize_window())
        });
        cx.on_action(|_: &Zoom, cx| defer_in_active_window(cx, |window, _| window.zoom_window()));
        cx.on_action(|_: &ToggleFullScreen, cx| {
            // Android: the window always fills the screen; the item shows
            // or hides the system's bars
            #[cfg(target_os = "android")]
            corvene_platform::android::toggle_full_screen();
            defer_in_active_window(cx, |window, _| window.toggle_fullscreen());
        });
        // `--hidden` (flag `406-launch-hidden`, Corvene addition): start with
        // the window ordered out, as after ⌘W; the Dock icon shows it
        let launch_hidden = hidden_argument(std::env::args())
            && corvene_core::AppState::global(cx)
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::LAUNCH_HIDDEN);
        if launch_hidden {
            info!("--hidden: the main window starts hidden");
            #[cfg(target_os = "macos")]
            for handle in cx.windows() {
                handle
                    .update(cx, |_, window, cx| {
                        corvene_ui::native_window::hide_window(window, cx)
                    })
                    .ok();
            }
            #[cfg(not(target_os = "macos"))]
            cx.hide();
        } else if parity_run() {
            #[cfg(target_os = "macos")]
            for handle in cx.windows() {
                handle
                    .update(cx, |_, window, cx| {
                        corvene_ui::native_window::park_offscreen(window, cx)
                    })
                    .ok();
            }
        } else {
            cx.activate(true);
        }
    });
}

/// Open the window of workspace `workspace` (`429-multiple-windows`; the
/// main window at launch, then File › New Window and "Open in New
/// Window"): GHD's `BrowserWindow` options, the `Workspace` view, the
/// per-window observers (system theme, reduce motion, the close button) and
/// its entry in `corvene_ui::windows`. A later window cascades 22 px from
/// the focused one.
fn open_workspace_window(
    workspace: corvene_core::WorkspaceId,
    state: Entity<corvene_core::AppState>,
    sidebar_width: Pixels,
    started: Instant,
    cx: &mut App,
) {
    if corvene_ui::windows::for_workspace(workspace, cx).is_some()
        || state.read(cx).workspace(workspace).is_none()
    {
        return;
    }
    let first = corvene_ui::windows::count(cx) == 0;
    // Same size as the GitHub Desktop reference captures in .docs.
    let window_size = size(px(1367.), px(814.));
    // CORVENE_WINDOW_SIZE=<width>x<height> (build with `--features
    // snapshots`): another size, also below the minimum, for snapshots
    // of the compact layout phones get.
    #[cfg(feature = "snapshots")]
    let forced_size = std::env::var("CORVENE_WINDOW_SIZE").ok().and_then(|spec| {
        let (width, height) = spec.split_once('x')?;
        Some(size(px(width.parse().ok()?), px(height.parse().ok()?)))
    });
    #[cfg(not(feature = "snapshots"))]
    let forced_size = None::<Size<Pixels>>.filter(|_| false);
    let window_size = forced_size.unwrap_or(window_size);
    let cascaded = corvene_ui::windows::focused(cx).and_then(|entry| {
        entry
            .window
            .update(cx, |_, window, _| match window.window_bounds() {
                WindowBounds::Windowed(bounds) => Some(Bounds {
                    origin: bounds.origin + point(px(22.), px(22.)),
                    size: bounds.size,
                }),
                _ => None,
            })
            .ok()
            .flatten()
    });
    let bounds = match cascaded {
        Some(bounds) if !first => bounds,
        _ => Bounds::centered(None, window_size, cx),
    };
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Corvene".into()),
            // macOS: hiddenInset; Linux keeps the window manager's frame
            // (Electron's default there)
            // Windows: no frame either, GHD draws its own title bar
            // there (`corvene_ui::title_bar_windows`)
            appears_transparent: cfg!(any(target_os = "macos", windows)),
            traffic_light_position: Some(point(px(9.), px(9.))),
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // GHD's 960 × 660; `407-smaller-minimum-sizes`: 600 × 400;
        // `420-min-size-fits-display`: never more than the primary
        // display's visible area (GHD's minimum can exceed a small
        // screen, `main-process/app-window.ts` `minWidth` / `minHeight`)
        window_min_size: Some(if let Some(forced) = forced_size {
            forced
        } else {
            let flags = &state.read(cx).flags;
            let min = if flags.bool(corvene_core::flags::ids::SMALLER_MINIMUM_SIZES) {
                size(px(600.), px(400.))
            } else {
                size(px(960.), px(660.))
            };
            match cx.primary_display() {
                Some(display) if flags.bool(corvene_core::flags::ids::MIN_SIZE_FITS_DISPLAY) => {
                    min.min(&display.visible_bounds().size)
                }
                _ => min,
            }
        }),
        app_id: Some(corvene_platform::BUNDLE_ID.into()),
        // X11 `_NET_WM_ICON` (Electron sets the app icon on its window)
        #[cfg(not(target_os = "macos"))]
        icon: corvene_ui::title_bar::window_icon(),
        ..Default::default()
    };
    #[cfg(not(target_os = "macos"))]
    let opened = {
        let workspace_slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = workspace_slot.clone();
        let state = state.clone();
        gpui_kit::open_window(options, cx, move |window, cx| {
            let view = cx.new(|cx| Workspace::new(state, workspace, sidebar_width, window, cx));
            *slot.borrow_mut() = Some(view.clone());
            cx.new(|cx| corvene_ui::menu_bar::MenuBarShell::new(view.into(), cx))
        })
        .map(|(handle, _)| (handle, workspace_slot.borrow_mut().take()))
    };
    #[cfg(target_os = "macos")]
    let opened = {
        let state = state.clone();
        gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| Workspace::new(state, workspace, sidebar_width, window, cx))
        })
        .map(|(handle, view)| (handle, Some(view)))
    };
    let (handle, view): (AnyWindowHandle, Entity<Workspace>) = match opened {
        Ok((handle, Some(view))) => {
            info!(
                elapsed_ms = started.elapsed().as_millis(),
                %workspace,
                "window opened"
            );
            (handle, view)
        }
        Ok((_, None)) => {
            error!("the window opened without a workspace");
            return;
        }
        Err(err) => {
            error!(?err, "failed to open window");
            return;
        }
    };
    corvene_ui::windows::register(handle, workspace, view.clone(), cx);

    // System theme follows macOS light/dark switches (`supportsSystemThemeChanges`)
    // and, back in Corvene, the "Increase contrast" display option.
    handle
        .update(cx, |_, window, cx| {
            view.update(cx, |_, cx| {
                cx.observe_window_activation(window, |_, window, cx| {
                    if window.is_window_active() {
                        sync_reduce_motion(cx);
                        sync_keyboard_navigation(cx);
                    }
                    let theme = APPLIED_THEME.with(|t| t.get());
                    if window.is_window_active()
                        && theme == ThemeSetting::System
                        && resolve_theme(theme, cx).name
                            != corvene_ui::theme::ActiveGhdTheme::ghd(&**cx).name
                    {
                        apply_theme(theme, cx);
                    }
                })
                .detach();
            });
            // the red close button hides the last window like ⌘W (GHD
            // `window.on('close')` → `hide()` unless quitting); another
            // window closes for good (`429-multiple-windows`)
            window.on_window_should_close(cx, |window, cx| {
                if corvene_ui::windows::count(cx) > 1 {
                    return true;
                }
                #[cfg(target_os = "macos")]
                {
                    corvene_ui::native_window::hide_window(window, cx);
                    false
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = (window, cx);
                    true
                }
            });
            window
                .observe_window_appearance(|_, cx| {
                    let theme = APPLIED_THEME.with(|t| t.get());
                    if theme == ThemeSetting::System {
                        apply_theme(theme, cx);
                    }
                })
                .detach();
            // a window opened after launch comes forward
            if !first {
                window.activate_window();
            }
        })
        .ok();
}

/// A parity harness run (`CORVENE_CONTROL`, `tools/parity`): the harness
/// injects input and renders offscreen, so Corvene never activates itself
/// and parks its window out of sight, leaving the keyboard with whatever
/// the user is typing into. `CORVENE_FOREGROUND=1` (the harness's
/// `PARITY_FOREGROUND=1`) keeps the usual launch.
fn parity_run() -> bool {
    cfg!(feature = "snapshots")
        && std::env::var_os("CORVENE_CONTROL").is_some()
        && std::env::var_os("CORVENE_FOREGROUND").is_none()
}

thread_local! {
    /// The theme setting last applied (the saved one, or `CORVENE_THEME`).
    static APPLIED_THEME: std::cell::Cell<ThemeSetting> =
        const { std::cell::Cell::new(ThemeSetting::System) };
}

/// The palette for `setting` given the system appearance, Settings ›
/// Accessibility › Display › "Increase contrast" and the
/// `101-high-contrast-theme` flag (read from the app state).
fn resolve_theme(setting: ThemeSetting, cx: &App) -> corvene_ui::theme::GhdTheme {
    let state = corvene_core::AppState::try_global(cx);
    let high_contrast = state.as_ref().is_none_or(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::HIGH_CONTRAST_THEME)
    });
    let variants = state
        .map(|s| corvene_ui::theme::ThemeVariants::of(&s.read(cx).flags))
        .unwrap_or_default();
    resolve_theme_with(setting, high_contrast, variants, cx)
}

/// `614-system-reduce-motion`: spinners and smooth scrolling follow the
/// system's Reduce Motion setting (read again whenever the window is
/// activated). Returns the flag's value.
fn sync_reduce_motion(cx: &mut App) -> bool {
    let on = corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::SYSTEM_REDUCE_MOTION)
    });
    cx.set_reduce_motion(on && corvene_platform::accessibility::reduce_motion());
    on
}

/// `622-system-keyboard-navigation`: Tab reaches buttons, checkboxes and
/// the like only with macOS's Keyboard navigation on (GHD: always). Read
/// again when a window becomes active, as macOS posts no change.
fn sync_keyboard_navigation(cx: &mut App) -> bool {
    let on = corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::SYSTEM_KEYBOARD_NAVIGATION)
    });
    // CORVENE_KEYBOARD_NAVIGATION=0|1 stands in for the macOS setting
    let system = match std::env::var("CORVENE_KEYBOARD_NAVIGATION").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => corvene_platform::accessibility::full_keyboard_access(),
    };
    corvene_ui::keyboard_nav::set_controls_reachable(!on || system, cx);
    on
}

/// `resolve_theme` before the app state exists: with `high_contrast` off a
/// High Contrast setting shows as Dark and "Increase contrast" is ignored.
fn resolve_theme_with(
    setting: ThemeSetting,
    high_contrast: bool,
    variants: corvene_ui::theme::ThemeVariants,
    cx: &App,
) -> corvene_ui::theme::GhdTheme {
    let system_dark = matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    corvene_ui::theme::GhdTheme::for_setting(
        corvene_core::flags::effective_theme(setting, high_contrast),
        system_dark,
        high_contrast && corvene_platform::accessibility::increase_contrast(),
    )
    .with_variants(variants)
}

/// Runs `f` on the workspace in its window once GPUI has handed that window
/// back. A shortcut's handler runs while GPUI updates the window the key
/// went to, and a menu item's does too (`Window::dispatch_action`): the
/// window is out of the app until the dispatch returns, so updating it
/// from the handler fails and the shortcut did nothing.
fn defer_in_focused_workspace(
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) {
    cx.defer(move |cx| {
        // `429-multiple-windows`: the key window's workspace
        let Some(entry) = corvene_ui::windows::focused(cx) else {
            return;
        };
        let workspace = entry.view;
        cx.with_window(workspace.entity_id(), |window, cx| {
            workspace.update(cx, |w, cx| f(w, window, cx))
        });
    });
}

/// Update the key window's workspace view (no window needed, so no
/// deferral: the entity is free while its window is on the update stack).
fn with_focused_workspace(cx: &mut App, f: impl FnOnce(&mut Workspace, &mut Context<Workspace>)) {
    if let Some(entry) = corvene_ui::windows::focused(cx) {
        entry.view.update(cx, f);
    }
}

/// [`defer_in_workspace`] for the window commands (Close Window, Minimize,
/// Zoom, Toggle Full Screen), which act on whichever window is active.
fn defer_in_active_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        // the active window, else the focused workspace's (the parity
        // harness's windows are never active)
        if let Some(entry) = corvene_ui::windows::focused(cx) {
            entry.window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

/// Registers the handler of a menu item GitHub Desktop disables while a
/// popup is open (`menu-update.ts` `getMenuState` → `allMenuIds`): with a
/// dialog up, its shortcut or menu item does nothing instead of acting on the
/// window behind it. The keymap binds these shortcuts in `!Popup`, so a text
/// field in the dialog keeps the key (⌘⌫ deletes to the line start).
fn on_menu_action<A: Action>(cx: &mut App, f: impl Fn(&A, &mut App) + 'static) {
    cx.on_action(move |action: &A, cx| {
        if corvene_core::AppState::global(cx)
            .read(cx)
            .popup()
            .is_none()
        {
            f(action, cx)
        }
    });
}

/// [`on_menu_action`] for an item `menu_state` can keep enabled under a
/// popup (`424-conflicts-dialog-keeps-open-items`: the open items under the
/// merge-conflicts dialog).
fn on_kept_menu_action<A: Action>(cx: &mut App, id: MenuId, f: impl Fn(&A, &mut App) + 'static) {
    cx.on_action(move |action: &A, cx| {
        let state = corvene_core::AppState::global(cx).read(cx);
        if state.popup().is_none() || corvene_core::menu_state::is_enabled(state, id) {
            f(action, cx)
        }
    });
}

/// `focus_main_window` for the dispatcher's host callbacks.
fn focus_main_window_host(cx: &mut dyn corvene_core::Host) {
    if let Some(cx) = cx.gpui_app() {
        focus_main_window(cx);
    }
}

/// GHD `focusWindow`: bring Corvene forward and show its window, even when
/// it was hidden with ⌘W.
fn focus_main_window(cx: &mut App) {
    if parity_run() {
        return;
    }
    cx.activate(true);
    #[cfg(target_os = "macos")]
    for handle in cx.windows() {
        handle
            .update(cx, |_, window, cx| {
                corvene_ui::native_window::show_window(window, cx)
            })
            .ok();
    }
    // `429-multiple-windows`: the focused window on top
    if let Some(entry) = corvene_ui::windows::focused(cx) {
        entry
            .window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    }
}

/// `CORVENE_POPUP=<name>` (and the parity harness's `popup` hook): open a
/// dialog or sample state for visual checks. Names are listed in `main`.
fn open_dev_popup(popup: &str, cx: &mut App) {
    let selected = corvene_core::AppState::global(cx).read(cx).selected;
    match (popup, selected) {
        ("import-ghd", _) => Dispatcher::show_popup(Popup::ImportFromGitHubDesktop, cx),
        ("repository-list", _) => Dispatcher::toggle_foldout(corvene_core::Foldout::Repository, cx),
        ("remove-repositories", ticked) => {
            Dispatcher::show_popup(Popup::RemoveRepositories { ticked }, cx)
        }
        (other, _) if other == "git-error" || other.starts_with("git-error:") => {
            let (title, err) = match other.strip_prefix("git-error:") {
                Some("raw") => (
                    "Could not push",
                    corvene_git::GitError::Failed {
                        args: "-c credential.helper=manager push --progress origin main:main"
                            .into(),
                        code: Some(128),
                        stderr: "fatal: unable to access 'https://github.com/octocat/spoon-knife.git/': \
                                 Failed to connect to github.com port 443 after 75004 ms: \
                                 Couldn't connect to server"
                            .into(),
                    },
                ),
                Some("known") => (
                    "Could not merge",
                    corvene_git::GitError::Failed {
                        args: "merge --no-ff topic".into(),
                        code: Some(128),
                        stderr: "fatal: refusing to merge unrelated histories".into(),
                    },
                ),
                Some("push") => (
                    "Could not push",
                    corvene_git::GitError::Failed {
                        args: "push --progress origin main:main".into(),
                        code: Some(1),
                        stderr: "remote: error: GH006: Protected branch update failed for refs/heads/main.\n\
                                 remote: \n\
                                 remote: - Changes must be made through a pull request.\n\
                                 remote: - 2 of 2 required status checks are expected.\n\
                                 To github.com:octocat/spoon-knife.git\n \
                                 ! [remote rejected] main -> main (protected branch hook declined)\n\
                                 error: failed to push some refs to 'github.com:octocat/spoon-knife.git'"
                            .into(),
                    },
                ),
                Some("plain") => (
                    "Could not pull",
                    corvene_git::GitError::Gix("The repository has no remotes.".into()),
                ),
                _ => (
                    "Could not pull",
                    corvene_git::GitError::Failed {
                        args: "-c rebase.backend=merge pull --ff --recurse-submodules --progress origin"
                            .into(),
                        code: Some(1),
                        stderr: "Updating e9455681..1f7e3d4e\n\
                                 error: Your local changes to the following files would be overwritten by merge:\n\
                                 \tcrates/corvene-platform/src/editors.rs\n\
                                 \tcrates/corvene-ui/src/dialogs/preferences.rs\n\
                                 \tcrates/corvene-ui/src/menu_bar.rs\n\
                                 \tcrates/corvene-ui/src/views_menu.rs\n\
                                 \tcrates/corvene-ui/src/widgets.rs\n\
                                 Please commit your changes or stash them before you merge.\n\
                                 Aborting"
                            .into(),
                    },
                ),
            };
            Dispatcher::show_error(title, &err, cx)
        }
        (other, _) if other == "flags" || other.starts_with("flags:") => {
            Dispatcher::open_flags(other.strip_prefix("flags:").map(str::to_string), cx)
        }
        (other, _)
            if other == "language-extensions" || other.starts_with("language-extensions:") =>
        {
            use corvene_core::extensions::ExtensionsFocus;
            let consent = other.ends_with(":consent");
            let focus = match other.strip_prefix("language-extensions:") {
                Some("consent") => None,
                Some("find") => Some(ExtensionsFocus::Find),
                Some("import") => Some(ExtensionsFocus::Import),
                // `117-file-icons`
                Some("icons") => Some(ExtensionsFocus::IconThemes),
                Some(rest) => rest
                    .strip_prefix("find=")
                    .map(|suffix| ExtensionsFocus::Suffix(suffix.to_string())),
                None => None,
            };
            Dispatcher::open_language_extensions(focus, None, cx);
            if consent {
                // the extensions load in the background: ask once they have
                cx.spawn(async move |cx: &mut AsyncApp| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(1500))
                        .await;
                    cx.update(|cx| Dispatcher::request_first_grammar_build(cx));
                })
                .detach();
            }
        }
        (other, _) if other.starts_with("preferences") => {
            use corvene_core::PreferencesTab as Tab;
            let tab = match other.strip_prefix("preferences:") {
                Some("integrations") => Tab::Integrations,
                Some("git") => Tab::Git,
                Some("appearance") => Tab::Appearance,
                Some("notifications") => Tab::Notifications,
                Some("prompts") => Tab::Prompts,
                Some("advanced") => Tab::Advanced,
                Some("accessibility") => Tab::Accessibility,
                _ => Tab::Accounts,
            };
            Dispatcher::open_preferences(tab, cx)
        }
        ("repository-settings", Some(id)) => Dispatcher::open_repository_settings(
            id,
            corvene_core::RepositorySettingsTab::Remote,
            cx,
        ),
        ("about", _) => Dispatcher::show_popup(
            Popup::About {
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            cx,
        ),
        ("create", _) => Dispatcher::show_popup(Popup::CreateRepository { path: None }, cx),
        ("clone", _) => Dispatcher::show_popup(Popup::CloneRepository { url: None }, cx),
        ("push-needs-pull", Some(id)) => {
            Dispatcher::show_popup(Popup::PushNeedsPull { repo: id }, cx)
        }
        ("initialize-lfs", Some(id)) => {
            Dispatcher::show_popup(Popup::InitializeLFS { repos: vec![id] }, cx)
        }
        // GHD `OversizedFiles` with two sample files (one of them tracked
        // for `1308-ignore-oversized-files`)
        ("oversized-files", Some(id)) => {
            let ignore = corvene_core::AppState::global(cx)
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::IGNORE_OVERSIZED_FILES);
            Dispatcher::show_popup(
                Popup::OversizedFiles {
                    repo: id,
                    files: vec!["assets/intro-video.mp4".into(), "data/dump.bin".into()],
                    summary: "Add the assets".into(),
                    description: String::new(),
                    lfs_patterns: Vec::new(),
                    ignore_tracked: ignore.then(|| vec!["data/dump.bin".into()]),
                    checks: Default::default(),
                },
                cx,
            )
        }
        // GHD `showFakeUpstreamAlreadyExists` (test UI components):
        // an in-memory fork of desktop/desktop whose `upstream`
        // points elsewhere
        ("upstream-already-exists", Some(id)) => {
            let parent = corvene_core::GitHubRepository {
                endpoint: "https://api.github.com".into(),
                owner: "desktop".into(),
                name: "desktop".into(),
                html_url: "https://github.com/desktop/desktop".into(),
                clone_url: "https://github.com/desktop/desktop.git".into(),
                default_branch: Some("development".into()),
                private: false,
                fork: false,
                parent: None,
                archived: false,
                permissions: None,
                allow_forking: None,
                node_id: None,
            };
            corvene_core::AppState::global(cx).update(cx, |s, _| {
                if let Some(r) = s.repositories.iter_mut().find(|r| r.id == id) {
                    let mut fork = parent.clone();
                    fork.owner = "octocat".into();
                    fork.fork = true;
                    fork.parent = Some(Box::new(parent));
                    r.github = Some(fork);
                }
            });
            Dispatcher::show_popup(
                Popup::UpstreamAlreadyExists {
                    repo: id,
                    existing_url: "https://github.com/someone-else/desktop.git".into(),
                },
                cx,
            )
        }
        // GHD test UI components: `MoveToApplicationsFolder`
        ("move-to-applications", _) => Dispatcher::show_popup(Popup::MoveToApplicationsFolder, cx),
        // GHD `showFakeReleaseNotes` (test UI components)
        ("release-notes", _) => {
            use corvene_core::release_notes::{parse_release_body, release_summary};
            let body = "Corvene now reads more of **GitHub Desktop**'s workflow: \
                    _Markdown_ with `inline code`, ~~webviews~~ and \
                    [links](https://github.com/wasi-master/corvene).\n\n\
                    - [New] Branch autocompletion in Add Worktree\n\
                    - [Improved] Clone resolves owner/name through the API\n\
                    - [Added] Commit message templates\n\
                    - [Fixed] Arrow keys scroll the changes list. Thanks @octocat!\n\
                    - [Fixed] Upstream remote conflicts are reported\n\
                    - [Removed] Stale menu items";
            Dispatcher::show_popup(
                Popup::ReleaseNotes {
                    summary: release_summary(
                        env!("CARGO_PKG_VERSION"),
                        Some(std::time::SystemTime::now()),
                        parse_release_body(body),
                    ),
                },
                cx,
            )
        }
        // GHD `TestNotifications`: the pull request notification
        // dialogs for the CORVENE_ADD_REPO repository
        (other, Some(id)) if other.starts_with("pr-review") => {
            use corvene_github::api::ApiPullRequestReviewState as State;
            let state = match other.strip_prefix("pr-review") {
                Some(":approved") => State::Approved,
                Some(":commented") => State::Commented,
                _ => State::ChangesRequested,
            };
            Dispatcher::show_popup(
                Popup::PullRequestReview {
                    repo: id,
                    pull_request: dev_samples::pull_request(id, cx),
                    review: dev_samples::review(state),
                    should_checkout_branch: true,
                    should_change_repository: false,
                },
                cx,
            )
        }
        (other, Some(id)) if other.starts_with("tutorial:") => {
            let step = corvene_core::tutorial::TutorialStep::parse(&other["tutorial:".len()..]);
            corvene_core::AppState::global(cx).update(cx, |s, cx| {
                if let Some(r) = s.repositories.iter_mut().find(|r| r.id == id) {
                    r.is_tutorial_repository = true;
                }
                s.tutorial_step_override = step;
                cx.notify();
            });
        }
        ("tutorial-create", _) => Dispatcher::show_create_tutorial_repository(cx),
        ("tutorial-exit", _) => Dispatcher::show_popup(Popup::ConfirmExitTutorial, cx),
        ("crash-report", _) => {
            Dispatcher::update_settings(cx, |s| s.save_crash_reports = true);
            Dispatcher::sync_crash_reports_setting(cx);
            panic!("CORVENE_POPUP=crash-report: deliberate crash");
        }
        ("no-write-access", Some(id)) => dev_samples::make_read_only(id, cx),
        (other, Some(id)) if other.starts_with("notification-click:") => {
            use corvene_core::notifications::TestNotificationType as Kind;
            let kind = match &other["notification-click:".len()..] {
                "comment" => Kind::PullRequestComment,
                "checks-failed" => Kind::ChecksFailed,
                _ => Kind::PullRequestReview,
            };
            let notification = corvene_core::samples::notification(kind, id, cx);
            if let Some(payload) = Dispatcher::notification_payload(&notification) {
                Dispatcher::notification_payload_clicked(&payload, cx);
            }
        }
        (other, _) if other.starts_with("update-available") => {
            let flags: Vec<&str> = other.split(':').skip(1).collect();
            let manager = if flags.contains(&"brew") {
                Some(corvene_core::PackageManager::Homebrew)
            } else if flags.contains(&"pkg") {
                Some(corvene_core::PackageManager::System)
            } else {
                None
            };
            Dispatcher::install_sample_update(manager, cx);
            if flags.contains(&"about") {
                Dispatcher::show_popup(
                    Popup::About {
                        version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    cx,
                );
            } else if flags.contains(&"notes") {
                Dispatcher::show_update_release_notes(cx);
            }
        }
        ("zoom-in", _) => cx.dispatch_action(&ZoomIn),
        ("zoom-out", _) => cx.dispatch_action(&ZoomOut),
        ("zoom-reset", _) => cx.dispatch_action(&ResetZoom),
        // GHD `simulateAliveEvent`: an Alive event through the real handler
        (other, Some(id)) if other.starts_with("alive:") => {
            use corvene_core::notifications::TestNotificationType as Kind;
            let parts: Vec<&str> = other.split(':').collect();
            let kind = match parts.get(1).copied() {
                Some("comment") => Kind::PullRequestComment,
                Some("checks-failed") => Kind::ChecksFailed,
                _ => Kind::PullRequestReview,
            };
            let data = if parts.contains(&"api") {
                corvene_core::AliveEventData::Api
            } else {
                corvene_core::AliveEventData::Sample
            };
            dev_samples::install_pull_requests(id, cx);
            Dispatcher::simulate_alive_event(id, kind, data, cx);
        }
        // `354-pull-request-event-notifications`: a sample event notification
        (other, _) if other.starts_with("pr-event:") => {
            Dispatcher::simulate_pull_request_event_notification(&other["pr-event:".len()..], cx);
        }
        // the sign-in dialog (device flow by default, browser flow link)
        ("sign-in", _) => Dispatcher::show_popup(Popup::SignIn { enterprise: false }, cx),
        ("sign-in-enterprise", _) => Dispatcher::show_popup(Popup::SignIn { enterprise: true }, cx),
        // flags 342-344: the GitLab / Gitea / Bitbucket sign-in dialog
        ("sign-in-gitlab", _) => Dispatcher::show_host_sign_in(corvene_core::HostKind::GitLab, cx),
        ("sign-in-gitea", _) => Dispatcher::show_host_sign_in(corvene_core::HostKind::Gitea, cx),
        ("sign-in-bitbucket", _) => {
            Dispatcher::show_host_sign_in(corvene_core::HostKind::Bitbucket, cx)
        }
        // `GenericGitAuthentication` after a failed fetch (`:user` with the
        // login known, so only the password is asked for)
        (name @ ("generic-git-auth" | "generic-git-auth:user"), Some(id)) => {
            Dispatcher::show_popup(
                Popup::GenericGitAuthentication {
                    repo: id,
                    remote_url: "https://git.example.com/octocat/spoon-knife.git".into(),
                    host: "git.example.com".into(),
                    username: name.ends_with(":user").then(|| "octocat".into()),
                    retry: corvene_core::RetryAction::Fetch,
                },
                cx,
            )
        }
        // `528-proxy-credentials`: a proxy wants a username and password
        // (`:user` with a saved username whose password was refused,
        // `:fetch` with a fetch of the selected repository to retry)
        (name, selected) if name == "proxy-auth" || name.starts_with("proxy-auth:") => {
            let parts: Vec<&str> = name.split(':').skip(1).collect();
            let retry = match selected {
                Some(id) if parts.contains(&"fetch") => corvene_core::proxy::ProxyRetry::Remote {
                    repo: id,
                    action: corvene_core::RetryAction::Fetch,
                },
                _ => corvene_core::proxy::ProxyRetry::None,
            };
            let user = parts.contains(&"user");
            Dispatcher::show_popup(
                Popup::ProxyAuthentication {
                    proxy: "proxy.example.com:3128".into(),
                    username: user.then(|| "octocat".into()),
                    rejected: user,
                    retry,
                },
                cx,
            )
        }
        ("add-ssh-host", _) => Dispatcher::show_popup(
            Popup::AddSshHost {
                request: 0,
                info: corvene_git::AddSshHostInfo {
                    host: "example.com".into(),
                    ip: "192.0.2.10".into(),
                    key_type: "ED25519".into(),
                    fingerprint: "SHA256:AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsab".into(),
                },
            },
            cx,
        ),
        ("ssh-user-password", _) => Dispatcher::show_popup(
            Popup::SshSecret {
                request: 0,
                kind: corvene_core::askpass::ReplyTo::User,
                subject: "git@example.com".into(),
                wrong: false,
            },
            cx,
        ),
        ("ssh-key-passphrase-prompt", _) => Dispatcher::show_popup(
            Popup::SshSecret {
                request: 0,
                kind: corvene_core::askpass::ReplyTo::Key,
                subject: "/Users/octocat/.ssh/id_ed25519".into(),
                wrong: false,
            },
            cx,
        ),
        ("ssh-key-passphrase", _) => Dispatcher::show_popup(
            Popup::SshKeyPassphrase {
                path: "/Users/octocat/.ssh/id_ed25519".into(),
                wrong: false,
            },
            cx,
        ),
        ("test-notifications", Some(id)) => {
            Dispatcher::show_popup(Popup::TestNotifications { repo: id }, cx)
        }
        // `336-request-reviewers`: the sample pull requests, sample
        // collaborators (nothing fetched) and the dialog for #42
        ("request-reviewers", Some(id)) => {
            dev_samples::install_pull_requests(id, cx);
            corvene_core::AppState::global(cx).update(cx, |s, cx| {
                s.repo_state_mut(id).collaborators = Some(std::sync::Arc::new(
                    ["hubot", "monalisa", "octocat", "wasi-master"]
                        .map(String::from)
                        .to_vec(),
                ));
                cx.notify();
            });
            Dispatcher::show_popup(
                Popup::RequestReviewers {
                    repo: id,
                    number: 42,
                },
                cx,
            );
        }
        // `1216-recent-activity`
        ("recent-activity", Some(id)) => Dispatcher::show_recent_activity(id, cx),
        // `1110-repository-insights`
        ("insights", Some(id)) => Dispatcher::show_insights(id, cx),
        ("insights:all", Some(id)) => {
            Dispatcher::show_insights(id, cx);
            Dispatcher::set_insights_range(id, corvene_core::insights::InsightsRange::AllTime, cx);
        }
        // `1218-compare-refs`: `compare-refs[:<base>[:<head>[:...]]]`
        (other, Some(id)) if other == "compare-refs" || other.starts_with("compare-refs:") => {
            let mut parts = other.split(':').skip(1).map(str::to_string);
            let (base, head, dots) = (parts.next(), parts.next(), parts.next());
            match (base, head) {
                (Some(base), Some(head)) => {
                    let range = if dots.as_deref() == Some("...") {
                        corvene_core::ref_compare::RefRange::Symmetric
                    } else {
                        corvene_core::ref_compare::RefRange::Range
                    };
                    Dispatcher::compare_refs(id, base, head, range, false, cx);
                }
                (base, _) => {
                    let head = corvene_ui::history::current_ref(id, cx);
                    Dispatcher::show_compare_refs(id, base, head, cx);
                }
            }
        }
        // `1219-tag-manager`
        ("tags", Some(id)) => Dispatcher::show_tags(id, cx),
        // `1223-checkout-from-fork`: `checkout-from-fork[:<owner>[:<branch>]]`
        (other, Some(id))
            if other == "checkout-from-fork" || other.starts_with("checkout-from-fork:") =>
        {
            let mut parts = other.splitn(3, ':').skip(1).map(str::to_string);
            let (owner, branch) = (parts.next(), parts.next());
            Dispatcher::show_checkout_from_fork(id, owner, branch, cx)
        }
        // `350-ssh-key-helper`
        ("create-ssh-key", _) => Dispatcher::show_popup(Popup::CreateSshKey, cx),
        ("ssh-key-needs-scope", _) => Dispatcher::show_popup(
            Popup::SshKeyNeedsScope {
                endpoint: "https://api.github.com".into(),
                login: "octocat".into(),
                title: "Corvene".into(),
            },
            cx,
        ),
        // `1105-clean-untracked-files`: `clean-untracked[:ignored]`
        ("clean-untracked", Some(id)) => Dispatcher::show_clean_untracked_files(id, cx),
        ("clean-untracked:ignored", Some(id)) => {
            Dispatcher::show_clean_untracked_files(id, cx);
            Dispatcher::load_clean_preview(id, true, cx);
        }
        // `1111-submodules`, `1112-sparse-checkout`
        ("submodules", Some(id)) => Dispatcher::show_submodules(id, cx),
        ("sparse-checkout", Some(id)) => Dispatcher::show_sparse_checkout(id, cx),
        // `1113-lfs-locks`: `force-unlock:<path>`
        (other, Some(id)) if other.starts_with("force-unlock:") => {
            Dispatcher::request_force_unlock(id, other["force-unlock:".len()..].to_string(), cx)
        }
        // `1313-changelists`: `new-changelist[:<paths>]`, `changelists:<Name>=<paths>;…`
        ("new-changelist", Some(id)) => {
            let paths = corvene_core::AppState::global(cx)
                .read(cx)
                .repo_states
                .get(&id)
                .map(|rs| rs.selected_files.clone())
                .unwrap_or_default();
            Dispatcher::show_popup(
                Popup::NewChangelist {
                    repo: id,
                    paths,
                    edit: None,
                },
                cx,
            );
        }
        (other, Some(id)) if other.starts_with("new-changelist:") => {
            let paths = other["new-changelist:".len()..]
                .split(',')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
            Dispatcher::show_popup(
                Popup::NewChangelist {
                    repo: id,
                    paths,
                    edit: None,
                },
                cx,
            );
        }
        (other, Some(id)) if other.starts_with("changelists:") => {
            for spec in other["changelists:".len()..].split(';') {
                let Some((name, paths)) = spec.split_once('=') else {
                    continue;
                };
                let paths = paths
                    .split(',')
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect();
                Dispatcher::create_changelist(
                    id,
                    name.to_string(),
                    String::new(),
                    paths,
                    false,
                    cx,
                );
            }
        }
        // `1106-apply-patch`: `apply-patch:<path>`, relative to the repository
        (other, Some(id)) if other.starts_with("apply-patch:") => {
            let path = std::path::PathBuf::from(&other["apply-patch:".len()..]);
            let path = match corvene_core::AppState::global(cx).read(cx).repository(id) {
                Some(repo) if path.is_relative() => repo.path.join(path),
                _ => path,
            };
            Dispatcher::preview_patch_file(id, path, cx);
        }
        // `348-pull-request-review`: the review of a sample pull request
        // with sample threads (no GitHub behind it)
        ("pull-request-review", Some(id)) => {
            corvene_core::pull_request_review::install_samples(id, cx);
        }
        // `345-issues`: the view with sample issues, the New Issue… dialog
        ("issues", Some(id)) => {
            corvene_core::issues::install_samples(id, cx);
            Dispatcher::show_issues(id, cx);
        }
        ("new-issue", Some(id)) => {
            corvene_core::issues::install_samples(id, cx);
            Dispatcher::show_popup(Popup::NewIssue { repo: id }, cx);
        }
        // `351-actions`: the view with sample workflows, runs and jobs (no
        // API), and Run workflow for the sample Release workflow
        ("actions", repo) | ("run-workflow", repo) => {
            let github = corvene_core::github_actions::install_samples(repo, cx);
            Dispatcher::show_popup(
                Popup::Actions {
                    github: github.clone(),
                    repo,
                },
                cx,
            );
            if popup == "run-workflow" {
                Dispatcher::select_actions_workflow(Some(4), cx);
                Dispatcher::show_popup(
                    Popup::RunWorkflow {
                        github,
                        workflow: 4,
                    },
                    cx,
                );
            }
        }
        // `346-releases`: the view with sample releases, Create Release…
        ("releases", Some(id)) => {
            corvene_core::releases::install_samples(id, cx);
            Dispatcher::show_releases(id, cx);
        }
        (other, Some(id)) if other.starts_with("create-release") => {
            let tag = other
                .strip_prefix("create-release:")
                .map(str::to_string)
                .filter(|t| !t.is_empty());
            Dispatcher::show_create_release(id, tag, None, cx);
        }
        // `798-blame`: `blame:<path>` (working tree) or `blame:<path>@<rev>`
        (other, Some(id)) if other.starts_with("blame:") => {
            let arg = &other["blame:".len()..];
            let (path, rev) = match arg.rsplit_once('@') {
                Some((path, rev)) => (path, Some(rev.to_string())),
                None => (arg, None),
            };
            let target = corvene_core::blame::BlameTarget {
                path: path.to_string(),
                rev,
                line: None,
            };
            Dispatcher::show_blame(id, target, Vec::new(), cx);
        }
        ("pr-list", Some(id)) => {
            dev_samples::install_pull_requests(id, cx);
            Dispatcher::change_branches_tab(corvene_core::BranchesTab::PullRequests, cx);
            Dispatcher::toggle_foldout(corvene_core::Foldout::Branch, cx);
        }
        ("pr-comment", Some(id)) => Dispatcher::show_popup(
            Popup::PullRequestComment {
                repo: id,
                pull_request: dev_samples::pull_request(id, cx),
                comment: dev_samples::comment(),
                should_checkout_branch: true,
                should_change_repository: false,
            },
            cx,
        ),
        // `347-actions-job-logs`: the sample failed job's log, no API
        ("job-log", Some(id)) => {
            let github = corvene_core::samples::github_repository(id, cx);
            let check = dev_samples::failed_checks()
                .into_iter()
                .find(|c| c.job_steps.is_some())
                .unwrap_or_else(|| dev_samples::failed_checks().remove(0));
            Dispatcher::install_job_log(&github, check.id, &corvene_core::samples::job_log(), cx);
            Dispatcher::show_job_log(Some(id), github, check, None, cx);
        }
        ("pr-checks-failed", Some(id)) => Dispatcher::show_popup(
            Popup::PullRequestChecksFailed {
                repo: id,
                pull_request: dev_samples::pull_request(id, cx),
                checks: dev_samples::failed_checks(),
                should_change_repository: false,
            },
            cx,
        ),
        // `clone:<url>` opens the URL tab pre-filled
        (other, _) if other.starts_with("clone:") => Dispatcher::show_popup(
            Popup::CloneRepository {
                url: Some(other["clone:".len()..].to_string()),
            },
            cx,
        ),
        _ => {}
    }
}

/// Settings › Appearance › Theme: swap the palette live (`ApplicationTheme`).
fn apply_theme(setting: ThemeSetting, cx: &mut App) {
    APPLIED_THEME.with(|t| t.set(setting));
    corvene_ui::theme::apply(resolve_theme(setting, cx), cx);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// What `main` reads from the store before the first window.
struct LaunchStore {
    store: Arc<corvene_store::Store>,
    settings: corvene_core::Settings,
    flag_overrides: corvene_core::flags::FlagOverrides,
    flags_env: corvene_core::flags::EnvFlags,
    launch_flags: corvene_core::Flags,
    settings_file: Option<(
        corvene_core::settings_file::SettingsOverlay,
        corvene_core::flags::EnvFlags,
        Vec<String>,
    )>,
    store_fallback: Option<std::path::PathBuf>,
}

/// Open the store (a temporary one when it cannot be opened) and resolve
/// the settings and flags the launch needs. Runs on its own thread.
fn open_store(started: Instant) -> LaunchStore {
    let mut store_fallback = None;
    let store = match corvene_store::Store::open_in(corvene_platform::paths::app_support_dir()) {
        Ok(store) => Arc::new(store),
        Err(err) => {
            error!(
                ?err,
                "could not open settings store; falling back to a temporary one"
            );
            store_fallback = Some(corvene_platform::paths::app_support_dir().join("corvene.redb"));
            // per process, so several instances can fall back at once
            let tmp = std::env::temp_dir().join(format!("corvene-fallback-{}", std::process::id()));
            Arc::new(corvene_store::Store::open_in(tmp).expect("temporary store"))
        }
    };
    phase(started, "store file opened");
    let mut settings = store.settings().unwrap_or_default();
    // Feature flags: the stored preset + overrides, then CORVENE_FLAGS for
    // this session (bad entries are logged and skipped). Resolved here too
    // because the theme is applied before `AppState` exists.
    let flag_overrides = store.flags().unwrap_or_default();
    let (mut flags_env, flag_errors) = corvene_core::flags::env::from_env();
    for err in &flag_errors {
        warn!("{err}");
    }
    let mut launch_flags = corvene_core::Flags::resolve(&flag_overrides, &flags_env);
    // Corvene (`522-settings-file`): the settings file over the stored
    // settings, its flags under CORVENE_FLAGS
    let settings_file = launch_flags
        .bool(corvene_core::flags::ids::SETTINGS_FILE)
        .then(|| {
            corvene_core::settings_file::apply_at_launch(
                &corvene_platform::paths::settings_file(),
                &mut settings,
                &mut flags_env,
            )
        });
    if settings_file.is_some() {
        launch_flags = corvene_core::Flags::resolve(&flag_overrides, &flags_env);
    }
    // Corvene (`287-repository-list-backup`): a copy of the store from
    // before an update, and a banner when this session cannot save
    let list_backup = launch_flags.bool(corvene_core::flags::ids::REPOSITORY_LIST_BACKUP);
    if list_backup
        && store_fallback.is_none()
        && let Some(backup) =
            corvene_core::persistence::backup_on_version_change(&store, env!("CARGO_PKG_VERSION"))
    {
        info!(path = %backup.display(), "backed up the store from the previous version");
    }
    let store_fallback = store_fallback.filter(|_| list_backup);
    // `910-background-store-writes`: after the backup, which copies the file
    sync_store_flags(&store, &launch_flags);
    phase(started, "store opened");
    LaunchStore {
        store,
        settings,
        flag_overrides,
        flags_env,
        launch_flags,
        settings_file,
        store_fallback,
    }
}

/// Flag `910-background-store-writes`: commit store writes off the main
/// thread (a durable commit is an fsync).
fn sync_store_flags(store: &corvene_store::Store, flags: &corvene_core::Flags) {
    store.set_background_writes(flags.bool(corvene_core::flags::ids::BACKGROUND_STORE_WRITES));
}

/// Flags `908-opaque-depth-pass` and `909-damage-scissor`: the wgpu
/// renderer's (Linux, Android) overdraw switches, applied from the next frame.
fn sync_renderer_flags(flags: &corvene_core::Flags) {
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "android"))]
    {
        use corvene_core::flags::ids;
        gpui_wgpu::set_opaque_depth_pass(flags.bool(ids::OPAQUE_DEPTH_PASS));
        gpui_wgpu::set_damage_scissor(flags.bool(ids::DAMAGE_SCISSOR));
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "android")))]
    let _ = flags;
}

/// Milliseconds since the kernel started this process (macOS; `None`
/// elsewhere).
fn since_process_start_ms() -> Option<u128> {
    #[cfg(target_os = "macos")]
    {
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `proc_pidinfo` writes at most `size` bytes into `info`,
        // a plain C struct for which all zeroes is a valid value
        let info = unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let written = libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            );
            (written == size).then_some(info)
        }?;
        let start = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(info.pbi_start_tvsec)
            + std::time::Duration::from_micros(info.pbi_start_tvusec);
        std::time::SystemTime::now()
            .duration_since(start)
            .ok()
            .map(|d| d.as_millis())
    }
    #[cfg(not(target_os = "macos"))]
    None
}

fn phase(started: Instant, what: &str) {
    debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "startup: {what}"
    );
}

/// `--open-repo <path>` (or `--open-repo=<path>`) from the command line tool.
fn open_repo_argument(args: impl IntoIterator<Item = String>) -> Option<std::path::PathBuf> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--open-repo" {
            return args.next().map(std::path::PathBuf::from);
        }
        if let Some(path) = arg.strip_prefix("--open-repo=") {
            return Some(std::path::PathBuf::from(path));
        }
    }
    None
}

/// `--hidden`: launch without showing the main window.
fn hidden_argument(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().any(|arg| arg == "--hidden")
}

#[cfg(test)]
mod tests {
    use super::{hidden_argument, open_repo_argument};

    #[::core::prelude::v1::test]
    fn hidden_argument_forms() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(hidden_argument(args(&["corvene", "--hidden"])));
        assert!(!hidden_argument(args(&["corvene", "--open-repo", "/tmp"])));
    }

    // `gpui_kit::*` brings GPUI's `test` macro into scope; use the std one.
    #[::core::prelude::v1::test]
    fn open_repo_argument_forms() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            open_repo_argument(args(&["corvene", "--open-repo", "/tmp/repo"])),
            Some("/tmp/repo".into())
        );
        assert_eq!(
            open_repo_argument(args(&["corvene", "-psn_0_123", "--open-repo=/a b"])),
            Some("/a b".into())
        );
        assert_eq!(open_repo_argument(args(&["corvene"])), None);
        assert_eq!(open_repo_argument(args(&["corvene", "--open-repo"])), None);
    }
}
