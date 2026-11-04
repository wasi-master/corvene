//! Settings dialog (`ui/preferences/preferences.tsx` + the per-tab
//! components). 600 px wide, a 190 px vertical tab bar beside tab content at
//! least 500 px tall (GHD 3.6.7; 440 px in 3.6.6); Cancel / Save.
//!
//! Deviations from GHD 3.6.6: no Copilot tab, no Copilot prompt checkbox (flag
//! `512-copilot-prompt-omitted`; with it off the checkbox is shown and only
//! stored), no Hooks sub-tab under Git (hook environment loading is not
//! implemented), no
//! Usage section under Advanced (no telemetry; "Save crash reports locally"
//! and Optional components sit there instead, scrolling within 500 px), and
//! no Formatting section (behind a feature flag in GHD).
//! With flag `custom-editor-name` the custom editor form has a Name box used
//! in "Open in …" labels (GHD `CustomIntegrationForm` has path and arguments).
//! With flag `513-integration-app-icons` the editor and shell menus show the
//! applications' icons (GHD `integrations.tsx` lists plain `<option>`s).
//! With flag `871-branch-name-trailing-slash-quiet` the default branch name
//! box does not say "Will be saved as" for a lone trailing `/` or `.`
//! (GHD `ref-name-text-box.tsx` does).
//! With flag `514-default-clone-location` Advanced has a "Clone location"
//! folder picker (GHD has no setting; it remembers the last clone's parent
//! folder, `ui/lib/default-dir.ts`).
//! With flag `522-settings-file` Advanced names the settings file, the
//! settings it set, and has "Export Settings…" (`corvene_core::settings_file`).
//! With flag `350-ssh-key-helper` Integrations ends in the SSH key section
//! Android always has: the key in `~/.ssh`, Create SSH Key…, Add to
//! ssh-agent and Add to GitHub (`corvene_core::ssh_keys`; GHD has none).
//! With flag `527-multiple-accounts` Accounts lists every account of a host,
//! each with its own Sign Out, and Add GitHub.com Account / Add GitHub
//! Enterprise Account (GHD `ui/preferences/accounts.tsx` shows the one
//! account per endpoint `accounts-store.ts` keeps).

use std::path::Path;
use std::rc::Rc;

use corvene_core::{
    AppState, Dispatcher, Popup, PreferencesSave, PreferencesTab, Settings, SyntaxHighlighter,
    TAB_SIZE_DEFAULT, ThemeSetting, UncommittedChangesStrategy,
};
use corvene_platform::notifications::NotificationPermission;
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::icons::Octicon;
use crate::tab_bar::{TabModel, VerticalTab, tab_bar};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{
    Inline, ListRowA11y, SelectHandler, SelectItem, button, checkbox_row, code_ref, labeled,
    link_button, paragraph, radio, radio_row, section_heading, select_button, select_button_items,
    settings_description, text_box,
};

/// Settings › Advanced › "Save crash reports locally" (where
/// `corvene_platform::crash_reports` writes and looks).
#[cfg(target_os = "macos")]
const CRASH_REPORTS_DESCRIPTION: &str = "When Corvene crashes, a report is saved in \
     ~/Library/Logs/Corvene/crashes and pointed out at the next launch, together with macOS's \
     own crash reports. Reports never leave this Mac.";
#[cfg(windows)]
const CRASH_REPORTS_DESCRIPTION: &str = "When Corvene crashes, a report is saved in \
     %LOCALAPPDATA%\\Corvene\\crashes and pointed out at the next launch, together with \
     Windows's own crash reports. Reports never leave this computer.";
#[cfg(not(any(target_os = "macos", windows)))]
const CRASH_REPORTS_DESCRIPTION: &str = "When Corvene crashes, a report is saved in \
     ~/.local/state/corvene/crashes and pointed out at the next launch, together with the \
     system's own crash reports. Reports never leave this computer.";

/// Error messages start lowercase (they follow "could not …"); a sentence
/// of their own starts with a capital.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `isValidCustomIntegration`: the bundle id of a `.app` path (kept when the
/// path did not change, `mdls` is a subprocess).
fn bundle_id_for(path: &str, previous: Option<&corvene_core::CustomIntegration>) -> Option<String> {
    if !path.ends_with(".app") {
        return None;
    }
    match previous {
        Some(prev) if prev.path == path && prev.bundle_id.is_some() => prev.bundle_id.clone(),
        _ => corvene_platform::custom_integration::app_bundle_id(Path::new(path)),
    }
}

const TABS: [PreferencesTab; 8] = [
    PreferencesTab::Accounts,
    PreferencesTab::Integrations,
    PreferencesTab::Git,
    PreferencesTab::Appearance,
    PreferencesTab::Notifications,
    PreferencesTab::Prompts,
    PreferencesTab::Advanced,
    PreferencesTab::Accessibility,
];

const TAB_SIZES: [u32; 9] = [1, 2, 3, 4, 5, 6, 8, 10, 12];

/// GHD `Git` sub-tabs (Hooks omitted).
#[derive(Clone, Copy, PartialEq, Eq)]
enum GitTab {
    Author,
    DefaultBranch,
    Hooks,
}

/// `OtherEmailSelectValue`
const OTHER_EMAIL: &str = "Other";

/// What Advanced's proxy section shows (`528-proxy-credentials`).
fn proxy_snapshot(
    state: &Entity<AppState>,
    cx: &App,
) -> (
    Option<corvene_core::proxy::ProxyStatus>,
    Vec<(String, String)>,
) {
    let s = state.read(cx);
    let mut logins: Vec<(String, String)> = s
        .proxy
        .logins
        .iter()
        .map(|(proxy, user)| (proxy.clone(), user.clone()))
        .collect();
    logins.sort();
    (s.proxy.status.clone(), logins)
}

pub struct PreferencesDialog {
    state: Entity<AppState>,
    tab: PreferencesTab,
    /// `528-proxy-credentials`: Advanced's proxy line and saved logins as
    /// last drawn (the state is observed for these alone).
    proxy_seen: (
        Option<corvene_core::proxy::ProxyStatus>,
        Vec<(String, String)>,
    ),
    /// The Accounts tab's `dialog-preferred-focus` button shows its focus
    /// ring until a mouse press moves focus without `:focus-visible`.
    preferred_focus_visible: bool,
    git_tab: GitTab,
    /// Working copy of the settings; written back on Save.
    draft: Settings,
    name: Entity<InputState>,
    email: Entity<InputState>,
    default_branch: Entity<InputState>,
    /// Author email chosen from the account emails; `None` = "Other" (text box).
    email_choice: Option<String>,
    /// The git config arrived and the fields were filled from it.
    git_loaded: bool,
    /// `517-path-git-settings`: global `core.quotepath` / `core.longpaths`.
    quotepath: bool,
    longpaths: bool,
    /// `526-commit-signing`: the global signing settings as edited (the key
    /// in `signing_key`).
    signing: corvene_core::SigningConfig,
    signing_key: Entity<InputState>,
    /// GHD `Notifications` state: `getNotificationsPermission()` result.
    notification_permission: Option<NotificationPermission>,
    /// `CustomIntegrationForm` inputs (Integrations tab).
    custom_editor_path: Entity<InputState>,
    custom_editor_args: Entity<InputState>,
    /// The custom editor's menu name (flag `custom-editor-name`).
    custom_editor_name: Entity<InputState>,
    custom_shell_path: Entity<InputState>,
    custom_shell_args: Entity<InputState>,
    /// `525-account-commit-email`: each account's commit email box, by
    /// `account_commit_email_key`.
    commit_email_inputs: Vec<(String, Entity<InputState>)>,
}

impl PreferencesDialog {
    pub fn new(
        state: Entity<AppState>,
        tab: PreferencesTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // `350-ssh-key-helper`: a key made outside Corvene since
        corvene_platform::ssh_key::forget_public_key();
        // `528-proxy-credentials`: Advanced's proxy line
        if state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::PROXY_CREDENTIALS)
        {
            Dispatcher::refresh_proxy_status(cx);
        }
        let mut draft = state.read(cx).settings.clone();
        // `Integrations.componentDidMount`: with no editor (shell) found the
        // select shows the custom entry, and its form, from the start
        {
            let s = state.read(cx);
            if s.editors.is_empty() && !draft.use_custom_editor {
                draft.use_custom_editor = true;
                draft.custom_editor.get_or_insert_with(Default::default);
            }
            if s.shells.is_empty() && !draft.use_custom_shell {
                draft.use_custom_shell = true;
                draft.custom_shell.get_or_insert_with(Default::default);
            }
        }
        let name = cx.new(|cx| InputState::new(window, cx));
        let email = cx.new(|cx| InputState::new(window, cx));
        let default_branch = cx.new(|cx| InputState::new(window, cx));
        let signing_key = cx.new(|cx| InputState::new(window, cx).placeholder("Key ID or file"));
        for input in [&name, &email, &default_branch] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        let mut custom_input =
            |placeholder: &'static str, value: String, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        // `523-custom-editor-list`: the form edits the chosen custom editor
        let editor_list = state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CUSTOM_EDITOR_LIST);
        let editor = draft
            .chosen_custom_editor(editor_list)
            .cloned()
            .unwrap_or_default();
        let shell = draft.custom_shell.clone().unwrap_or_default();
        let custom_editor_path = custom_input("Path to executable", editor.path, cx);
        let custom_editor_args = custom_input("Command line arguments", editor.arguments, cx);
        let custom_editor_name = custom_input("Custom Editor", editor.name, cx);
        let custom_shell_path = custom_input("Path to executable", shell.path, cx);
        let custom_shell_args = custom_input("Command line arguments", shell.arguments, cx);
        for input in [
            &custom_editor_path,
            &custom_editor_args,
            &custom_editor_name,
        ] {
            cx.observe(input, |this, _, cx| {
                let index = this.custom_editor_index(cx);
                let mut editors = this.draft.custom_editors();
                let path = this.custom_editor_path.read(cx).value().trim().to_string();
                let bundle_id = bundle_id_for(&path, editors.get(index));
                let editor = corvene_core::CustomIntegration {
                    path,
                    arguments: this.custom_editor_args.read(cx).value().trim().to_string(),
                    bundle_id,
                    name: this.custom_editor_name.read(cx).value().trim().to_string(),
                };
                match editors.get_mut(index) {
                    Some(entry) => *entry = editor,
                    None => editors.push(editor),
                }
                this.draft.set_custom_editors(editors);
                cx.notify();
            })
            .detach();
        }
        for input in [&custom_shell_path, &custom_shell_args] {
            cx.observe(input, |this, _, cx| {
                let path = this.custom_shell_path.read(cx).value().trim().to_string();
                let bundle_id = bundle_id_for(&path, this.draft.custom_shell.as_ref());
                this.draft.custom_shell = Some(corvene_core::CustomIntegration {
                    path,
                    arguments: this.custom_shell_args.read(cx).value().trim().to_string(),
                    bundle_id,
                    name: String::new(),
                });
                cx.notify();
            })
            .detach();
        }
        // `525-account-commit-email`
        let commit_email_inputs: Vec<(String, Entity<InputState>)> = state
            .read(cx)
            .accounts
            .clone()
            .iter()
            .map(|account| {
                let key = corvene_core::dispatcher::account_commit_email_key(account);
                let value = draft
                    .account_commit_emails
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Leave the repository's email alone")
                        .default_value(value)
                });
                let observed = key.clone();
                cx.observe(&input, move |this, input, cx| {
                    let email = input.read(cx).value().trim().to_string();
                    if email.is_empty() {
                        this.draft.account_commit_emails.remove(&observed);
                    } else {
                        this.draft
                            .account_commit_emails
                            .insert(observed.clone(), email);
                    }
                    cx.notify();
                })
                .detach();
                (key, input)
            })
            .collect();
        crate::windows::observe_state_in(&state, window, cx, |this, state, window, cx| {
            this.fill_from_git_config(&state, window, cx);
            cx.notify();
        })
        .detach();
        let proxy_seen = proxy_snapshot(&state, cx);
        cx.observe(&state, |this: &mut Self, state, cx| {
            let seen = proxy_snapshot(&state, cx);
            if seen != this.proxy_seen {
                this.proxy_seen = seen;
                cx.notify();
            }
        })
        .detach();
        let mut this = Self {
            state: state.clone(),
            tab,
            proxy_seen,
            preferred_focus_visible: true,
            git_tab: GitTab::Author,
            draft,
            name,
            email,
            default_branch,
            email_choice: None,
            git_loaded: false,
            quotepath: true,
            longpaths: false,
            signing: corvene_core::SigningConfig::default(),
            signing_key,
            notification_permission: None,
            custom_editor_path,
            custom_editor_args,
            custom_editor_name,
            custom_shell_path,
            custom_shell_args,
            commit_email_inputs,
        };
        this.fill_from_git_config(&state, window, cx);
        this.poll_notification_permission(cx);
        this
    }

    /// `updateNotificationsState`: the centre answers on its own queue, so
    /// ask off the main thread.
    fn poll_notification_permission(&mut self, cx: &mut Context<Self>) {
        let task = cx
            .background_executor()
            .spawn(async move { corvene_platform::notifications::permission() });
        cx.spawn(async move |this, cx| {
            let permission = task.await;
            this.update(cx, |this, cx| {
                this.notification_permission = Some(permission);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `onGrantNotificationPermission`: show the system prompt, then poll
    /// until the status leaves `Default` (the prompt is asynchronous).
    fn grant_notification_permission(&mut self, cx: &mut Context<Self>) {
        corvene_platform::notifications::request_permission();
        cx.spawn(async move |this, cx| {
            for _ in 0..60 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let permission = cx
                    .background_executor()
                    .spawn(async move { corvene_platform::notifications::permission() })
                    .await;
                let done = this
                    .update(cx, |this, cx| {
                        this.notification_permission = Some(permission);
                        cx.notify();
                        permission != NotificationPermission::Default
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// `isLoadingGitConfig` → fields filled once the global config is read.
    fn fill_from_git_config(
        &mut self,
        state: &Entity<AppState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git_loaded {
            return;
        }
        let Some(config) = state.read(cx).global_git.clone() else {
            return;
        };
        self.git_loaded = true;
        self.quotepath = config.quotepath;
        self.longpaths = config.longpaths;
        self.signing = config.signing.clone();
        self.signing_key.update(cx, |s, cx| {
            s.set_value(config.signing.key.clone(), window, cx)
        });
        let name = config.name.unwrap_or_default();
        let email = config.email.unwrap_or_default();
        self.name.update(cx, |s, cx| s.set_value(name, window, cx));
        self.email
            .update(cx, |s, cx| s.set_value(email.clone(), window, cx));
        self.default_branch
            .update(cx, |s, cx| s.set_value(config.default_branch, window, cx));
        let emails = self.account_emails(cx);
        self.email_choice = emails.iter().find(|e| **e == email).cloned();
        if !emails.is_empty() && self.email_choice.is_none() && email.is_empty() {
            // GHD preselects the first account email when nothing is configured.
            self.email_choice = emails.first().cloned();
            if let Some(first) = emails.first() {
                self.email
                    .update(cx, |s, cx| s.set_value(first.clone(), window, cx));
            }
        }
    }

    fn account_emails(&self, cx: &App) -> Vec<String> {
        let s = self.state.read(cx);
        let mut out: Vec<String> = Vec::new();
        for account in &s.accounts {
            for email in &account.emails {
                if !out.iter().any(|e| e.eq_ignore_ascii_case(email)) {
                    out.push(email.clone());
                }
            }
        }
        out
    }

    fn save(&self, cx: &mut App) {
        let path_settings = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::PATH_GIT_SETTINGS);
        Dispatcher::save_preferences(
            PreferencesSave {
                settings: self.draft.clone(),
                name: self.name.read(cx).value().to_string(),
                email: self.email.read(cx).value().to_string(),
                default_branch: self.default_branch.read(cx).value().to_string(),
                quotepath: (self.git_loaded && path_settings).then_some(self.quotepath),
                longpaths: (self.git_loaded && path_settings && cfg!(windows))
                    .then_some(self.longpaths),
                // `526-commit-signing`
                signing: (self.git_loaded
                    && self
                        .state
                        .read(cx)
                        .flags
                        .bool(corvene_core::flags::ids::COMMIT_SIGNING))
                .then(|| corvene_core::SigningConfig {
                    key: self.signing_key.read(cx).value().trim().to_string(),
                    ..self.signing.clone()
                }),
            },
            cx,
        );
    }

    /// The custom editor the form edits: the chosen one with
    /// `523-custom-editor-list`, else GHD's only one.
    fn custom_editor_index(&self, cx: &App) -> usize {
        let list = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CUSTOM_EDITOR_LIST);
        if list {
            self.draft.custom_editor_index
        } else {
            0
        }
    }

    /// `523-custom-editor-list`: make custom editor `index` the one in use
    /// and show it in the form.
    fn choose_custom_editor(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.draft.use_custom_editor = true;
        self.draft.custom_editor_index = index;
        let editor = self
            .draft
            .custom_editors()
            .get(index)
            .cloned()
            .unwrap_or_default();
        self.custom_editor_path
            .update(cx, |s, cx| s.set_value(editor.path, window, cx));
        self.custom_editor_args
            .update(cx, |s, cx| s.set_value(editor.arguments, window, cx));
        self.custom_editor_name
            .update(cx, |s, cx| s.set_value(editor.name, window, cx));
        cx.notify();
    }

    /// `523-custom-editor-list`: drop the custom editor the form shows;
    /// with none left the first installed editor is used again.
    fn remove_custom_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.draft.custom_editor_index;
        let mut editors = self.draft.custom_editors();
        if index < editors.len() {
            editors.remove(index);
        }
        let left = editors.len();
        self.draft.set_custom_editors(editors);
        if left == 0 {
            self.draft.use_custom_editor = false;
            cx.notify();
        } else {
            self.choose_custom_editor(index.min(left - 1), window, cx);
        }
    }

    fn edit<F: Fn(&mut Settings, bool) + 'static>(
        &self,
        cx: &Context<Self>,
        apply: F,
    ) -> impl Fn(bool, &mut Window, &mut App) + 'static {
        let weak = cx.weak_entity();
        move |value, _, cx| {
            weak.update(cx, |this, cx| {
                apply(&mut this.draft, value);
                cx.notify();
            })
            .ok();
        }
    }

    // ---- tabs ----

    /// `525-account-commit-email`: `account`'s commit email box.
    fn commit_email_field(
        &self,
        account: &corvene_core::Account,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::ACCOUNT_COMMIT_EMAIL)
        {
            return None;
        }
        let key = corvene_core::dispatcher::account_commit_email_key(account);
        let (_, input) = self.commit_email_inputs.iter().find(|(k, _)| *k == key)?;
        Some(
            div()
                .mb(SPACING())
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .child(labeled(
                    "Commit email",
                    text_box(
                        SharedString::from(format!("prefs-commit-email-{key}")),
                        input,
                        None,
                        window,
                        cx,
                    ),
                    cx,
                ))
                .child(settings_description(cx).child(format!(
                    "Repositories of {} you clone or add get this email for their commits \
                     unless they have one.",
                    account.friendly_endpoint()
                )))
                .into_any_element(),
        )
    }

    fn accounts_tab(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let s = self.state.read(cx);
        // `527-multiple-accounts`: every GitHub.com account, each with its
        // own Sign Out, and Add account
        let multiple = s.multiple_accounts();
        let dotcom_accounts: Vec<corvene_core::Account> = if multiple {
            s.accounts
                .iter()
                .filter(|a| a.is_dotcom())
                .cloned()
                .collect()
        } else {
            s.dotcom_account().cloned().into_iter().collect()
        };
        let dotcom = dotcom_accounts.first().cloned();
        let enterprise: Vec<_> = s
            .accounts
            .iter()
            .filter(|a| !a.is_dotcom())
            .cloned()
            .collect();
        let account_row = |account: &corvene_core::Account, id: ElementId| {
            let endpoint = account.endpoint.clone();
            let login = account.login.clone();
            let (title, subtitle) = if account.is_dotcom() {
                (
                    account
                        .name
                        .clone()
                        .unwrap_or_else(|| account.login.clone()),
                    format!("@{}", account.login),
                )
            } else {
                let title = match &account.name {
                    Some(name) if name != &account.login => {
                        format!("@{} ({name})", account.login)
                    }
                    _ => format!("@{}", account.login),
                };
                (title, format!("https://{}", account.host()))
            };
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .mb(SPACING())
                .child(crate::widgets::avatar_image(
                    account
                        .avatar_url
                        .as_deref()
                        .and_then(|u| crate::widgets::avatar_lookup_url(u, cx)),
                    zpx(34.),
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(title),
                        )
                        .child(
                            div()
                                .text_color(t.text_secondary)
                                .truncate()
                                .child(subtitle),
                        ),
                )
                .child(
                    button(id, mac_or("Sign Out", "Sign out"), cx).on_click(move |_, _, cx| {
                        if multiple {
                            Dispatcher::sign_out_account(endpoint.clone(), login.clone(), cx)
                        } else {
                            Dispatcher::sign_out(endpoint.clone(), cx)
                        }
                    }),
                )
        };
        div()
            .flex()
            .flex_col()
            .child(section_heading("GitHub.com", cx))
            .child(match &dotcom {
                Some(_) => div()
                    .children(dotcom_accounts.iter().enumerate().map(|(ix, account)| {
                        let id = if ix == 0 {
                            ElementId::from("prefs-signout-dotcom")
                        } else {
                            ElementId::from(("prefs-signout-dotcom", ix))
                        };
                        div()
                            .child(account_row(account, id))
                            .children(self.commit_email_field(account, window, cx))
                    }))
                    .when(multiple, |d| {
                        d.child(
                            div().flex().mb(SPACING()).child(
                                button(
                                    "prefs-add-dotcom",
                                    mac_or("Add GitHub.com Account", "Add GitHub.com account"),
                                    cx,
                                )
                                .on_click(|_, _, cx| {
                                    Dispatcher::close_popup(cx);
                                    Dispatcher::show_add_account_dialog(false, cx)
                                }),
                            ),
                        )
                    })
                    .into_any_element(),
                None => accounts_call_to_action(
                    "prefs-signin-dotcom",
                    "Sign in to your GitHub.com account to access your repositories.",
                    mac_or("Sign Into GitHub.com", "Sign into GitHub.com"),
                    self.preferred_focus_visible,
                    // GHD `onDotComSignIn`: Settings closes first
                    |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::show_popup(Popup::SignIn { enterprise: false }, cx)
                    },
                    cx,
                )
                .mb(SPACING())
                .into_any_element(),
            })
            .child(section_heading("GitHub Enterprise", cx))
            .children(
                enterprise
                    .iter()
                    .enumerate()
                    .map(|(ix, account)| {
                        let id = if ix == 0 {
                            ElementId::from("prefs-signout-enterprise")
                        } else {
                            ElementId::from(("prefs-signout-enterprise", ix))
                        };
                        div()
                            .child(account_row(account, id))
                            .children(self.commit_email_field(account, window, cx))
                    }),
            )
            .child(if enterprise.is_empty() {
                accounts_call_to_action(
                    "prefs-signin-enterprise",
                    "If you are using GitHub Enterprise at work, sign in to it to get access to your repositories.",
                    mac_or("Sign Into GitHub Enterprise", "Sign into GitHub Enterprise"),
                    false,
                    |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::show_popup(Popup::SignIn { enterprise: true }, cx)
                    },
                    cx,
                )
                .into_any_element()
            } else if multiple {
                // `527-multiple-accounts`: like GitHub.com's Add button
                div()
                    .flex()
                    .mb(SPACING())
                    .child(
                        button(
                            "prefs-add-enterprise",
                            mac_or(
                                "Add GitHub Enterprise Account",
                                "Add GitHub Enterprise account",
                            ),
                            cx,
                        )
                        .on_click(|_, _, cx| {
                            Dispatcher::close_popup(cx);
                            Dispatcher::show_add_account_dialog(true, cx)
                        }),
                    )
                    .into_any_element()
            } else {
                button("prefs-add-enterprise", "Add GitHub Enterprise account", cx)
                    .on_click(|_, _, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::show_popup(Popup::SignIn { enterprise: true }, cx)
                    })
                    .into_any_element()
            })
            .children(
                corvene_core::HostKind::OTHERS
                    .into_iter()
                    .filter(|kind| s.host_kind_enabled(*kind))
                    .map(|kind| self.host_accounts_section(kind, cx)),
            )
            .into_any_element()
    }

    /// Flags 342-344: the GitLab, Gitea and Forgejo, and Bitbucket sections
    /// (Corvene addition), laid out like GitHub Enterprise's.
    fn host_accounts_section(&self, kind: corvene_core::HostKind, cx: &Context<Self>) -> Div {
        use corvene_core::HostKind;
        let t = cx.ghd();
        let accounts: Vec<corvene_core::HostAccount> = self
            .state
            .read(cx)
            .hosts
            .accounts
            .iter()
            .filter(|a| a.kind() == kind)
            .cloned()
            .collect();
        let (id, body, action, add_id, add_label): (
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            &'static str,
        ) = match kind {
            HostKind::Gitea => (
                "prefs-signin-gitea",
                "Sign in to Codeberg or your Gitea or Forgejo server to see pull requests and CI \
                 statuses of its repositories.",
                mac_or("Sign Into Gitea or Forgejo", "Sign into Gitea or Forgejo"),
                "prefs-add-gitea",
                "Add Gitea or Forgejo account",
            ),
            HostKind::Bitbucket => (
                "prefs-signin-bitbucket",
                "Sign in to Bitbucket to see pull requests and build statuses of your \
                 repositories there.",
                mac_or("Sign Into Bitbucket", "Sign into Bitbucket"),
                "prefs-add-bitbucket",
                "Add Bitbucket account",
            ),
            _ => (
                "prefs-signin-gitlab",
                "Sign in to GitLab.com or your GitLab server to see merge requests and pipelines \
                 of its repositories.",
                mac_or("Sign Into GitLab", "Sign into GitLab"),
                "prefs-add-gitlab",
                "Add GitLab account",
            ),
        };
        let row = |account: &corvene_core::HostAccount, ix: usize| {
            let api_base = account.endpoint.api_base.clone();
            let login = account.login.clone();
            let subtitle = if account.needs_reauth {
                format!("{} · signed out by the server", account.endpoint.web_base)
            } else {
                account.endpoint.web_base.clone()
            };
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING())
                .mb(SPACING())
                .child(crate::widgets::avatar_image(
                    account
                        .avatar_url
                        .as_deref()
                        .and_then(|u| crate::widgets::avatar_lookup_url(u, cx)),
                    zpx(34.),
                    cx,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(account.title()),
                        )
                        .child(
                            div()
                                .text_color(if account.needs_reauth {
                                    t.error
                                } else {
                                    t.text_secondary
                                })
                                .truncate()
                                .child(subtitle),
                        ),
                )
                .when(account.needs_reauth, |d| {
                    d.child(
                        button(
                            ("prefs-host-reauth", ix),
                            mac_or("Sign In Again", "Sign in again"),
                            cx,
                        )
                        .on_click(move |_, _, cx| {
                            Dispatcher::close_popup(cx);
                            Dispatcher::show_host_sign_in(kind, cx)
                        }),
                    )
                })
                .child(
                    button(
                        ("prefs-host-signout", ix),
                        mac_or("Sign Out", "Sign out"),
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        Dispatcher::host_sign_out(api_base.clone(), login.clone(), cx)
                    }),
                )
        };
        div()
            .flex()
            .flex_col()
            .child(section_heading(kind.section_title(), cx))
            .children(accounts.iter().enumerate().map(|(ix, a)| row(a, ix)))
            .child(if accounts.is_empty() {
                accounts_call_to_action(
                    id,
                    body,
                    action,
                    false,
                    move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::show_host_sign_in(kind, cx)
                    },
                    cx,
                )
                .mb(SPACING())
                .into_any_element()
            } else {
                div()
                    .flex()
                    .mb(SPACING())
                    .child(button(add_id, add_label, cx).on_click(move |_, _, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::show_host_sign_in(kind, cx)
                    }))
                    .into_any_element()
            })
    }

    /// `CustomIntegrationForm`: Path + Choose…, Arguments, validation messages.
    /// `name`: the custom editor's menu-name box (flag `custom-editor-name`).
    fn custom_form(
        &self,
        id: &'static str,
        path: &Entity<InputState>,
        args: &Entity<InputState>,
        name: Option<&Entity<InputState>>,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let t = cx.ghd();
        let path_text = path.read(cx).value().trim().to_string();
        let args_text = args.read(cx).value().trim().to_string();
        let path_error = (!path_text.is_empty()
            && !corvene_platform::custom_integration::path_looks_valid(&path_text))
        .then_some("This path does not appear to be a valid executable.");
        let args_error = if args_text.is_empty() {
            None
        } else {
            match corvene_platform::custom_integration::parse_arguments(&args_text) {
                None => Some("These arguments are not valid.".to_string()),
                Some(argv) if !corvene_platform::custom_integration::has_target_path(&argv) => {
                    Some(format!(
                        "Arguments must include the target path placeholder ({}).",
                        corvene_platform::custom_integration::TARGET_PATH_ARGUMENT
                    ))
                }
                Some(_) => None,
            }
        };
        let input_error = |message: String| {
            // `.input-description.input-description-error`
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .text_size(FONT_SIZE_SM())
                .text_color(t.form_error_text)
                .child(crate::icons::octicon(Octicon::Alert, t.input_icon_error).size(zpx(12.)))
                .child(message)
        };
        let path_for_choose = path.clone();
        div()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(SPACING())
                    .child(labeled(
                        "Path",
                        text_box(
                            SharedString::from(format!("{id}-path")),
                            path,
                            None,
                            window,
                            cx,
                        ),
                        cx,
                    ))
                    .child(
                        button(SharedString::from(format!("{id}-choose")), "Choose…", cx)
                            .flex_none()
                            .on_click(move |_, window, cx| {
                                // apps are directories on macOS, so allow both
                                let receiver = cx.prompt_for_paths(PathPromptOptions {
                                    files: true,
                                    directories: true,
                                    multiple: false,
                                    prompt: Some("Choose".into()),
                                });
                                let path = path_for_choose.clone();
                                window
                                    .spawn(cx, async move |cx| {
                                        if let Ok(Ok(Some(paths))) = receiver.await
                                            && let Some(p) = paths.into_iter().next()
                                        {
                                            path.update_in(cx, |s, window, cx| {
                                                s.set_value(p.display().to_string(), window, cx)
                                            })
                                            .ok();
                                        }
                                    })
                                    .detach();
                            }),
                    ),
            )
            .when_some(path_error, |d, message| {
                d.child(input_error(message.to_string()))
            })
            .child(labeled(
                "Arguments",
                text_box(
                    SharedString::from(format!("{id}-args")),
                    args,
                    None,
                    window,
                    cx,
                ),
                cx,
            ))
            .when_some(args_error, |d, message| d.child(input_error(message)))
            .when_some(name, |d, name| {
                d.child(labeled(
                    "Name",
                    text_box(
                        SharedString::from(format!("{id}-name")),
                        name,
                        None,
                        window,
                        cx,
                    ),
                    cx,
                ))
            })
    }

    fn integrations_tab(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let s = self.state.read(cx);
        let editors: Vec<SharedString> = s.editors.iter().map(|e| e.name.clone().into()).collect();
        let shells: Vec<SharedString> = s
            .shells
            .iter()
            .map(|s| SharedString::from(s.shell.label()))
            .collect();
        // the applications' icons: Android's launcher icons; elsewhere the
        // ones loaded with the integrations (flag `513-integration-app-icons`)
        let app_icon = |key: &str, path: &Path| {
            if cfg!(target_os = "android") {
                crate::widgets::app_icon(key)
            } else {
                s.app_icons
                    .get(path)
                    .map(|icon| crate::widgets::integration_icon(path, icon))
            }
        };
        let editor_icons: Vec<_> = s
            .editors
            .iter()
            .map(|e| app_icon(&e.bundle_id, &e.path))
            .collect();
        let shell_icons: Vec<_> = s
            .shells
            .iter()
            .map(|s| app_icon(&s.bundle_id, &s.path))
            .collect();
        let use_custom_editor = self.draft.use_custom_editor;
        let use_custom_shell = self.draft.use_custom_shell;
        // `CustomIntegrationValue`: the last option configures a custom integration.
        // Android: applications are intents, not executables with arguments,
        // so there is no custom integration to configure
        let android = cfg!(target_os = "android");
        // `523-custom-editor-list`: every custom editor by name, then "Add
        // Custom Editor…"
        let editor_list = s.flags.bool(corvene_core::flags::ids::CUSTOM_EDITOR_LIST) && !android;
        let custom_names: Vec<SharedString> = if editor_list {
            self.draft
                .custom_editors()
                .iter()
                .enumerate()
                .map(|(ix, e)| e.display_name(ix, true).into())
                .collect()
        } else {
            Vec::new()
        };
        let custom_index = self.custom_editor_index(cx);
        let mut editor_options = editors.clone();
        editor_options.extend(custom_names.iter().cloned());
        if editor_list {
            editor_options.push(mac_or("Add Custom Editor…", "Add custom editor…").into());
        } else if !android {
            editor_options
                .push(mac_or("Configure Custom Editor…", "Configure custom editor…").into());
        }
        let editor_value = if use_custom_editor {
            custom_names
                .get(custom_index)
                .map(|n| n.to_string())
                .unwrap_or_else(|| {
                    mac_or("Configure Custom Editor…", "Configure custom editor…").to_string()
                })
        } else {
            self.draft
                .external_editor
                .clone()
                .or_else(|| s.editors.first().map(|e| e.name.clone()))
                .unwrap_or_default()
        };
        let editor_ix = if use_custom_editor {
            Some(editors.len() + if editor_list { custom_index } else { 0 })
        } else {
            editors.iter().position(|e| e.as_ref() == editor_value)
        };
        let mut shell_options = shells.clone();
        if !android {
            shell_options.push(mac_or("Configure Custom Shell…", "Configure custom shell…").into());
        }
        let shell_value = if use_custom_shell {
            mac_or("Configure Custom Shell…", "Configure custom shell…").to_string()
        } else {
            self.draft
                .shell
                .clone()
                .unwrap_or_else(|| corvene_platform::shells::DEFAULT_SHELL.label().to_string())
        };
        let shell_ix = if use_custom_shell {
            Some(shells.len())
        } else {
            shells.iter().position(|e| e.as_ref() == shell_value)
        };
        let weak = cx.weak_entity();
        let editor_names = editors.clone();
        let custom_count = custom_names.len();
        let on_editor: SelectHandler = Rc::new(move |ix, window, cx| {
            let name = editor_names.get(ix).map(|n| n.to_string());
            let custom = ix >= editor_names.len();
            weak.update(cx, |this, cx| {
                if editor_list && custom {
                    // a listed custom editor, or a new one at the end
                    let index = ix - editor_names.len();
                    if index == custom_count {
                        let mut editors = this.draft.custom_editors();
                        editors.push(corvene_core::CustomIntegration::default());
                        this.draft.set_custom_editors(editors);
                    }
                    this.choose_custom_editor(index, window, cx);
                    return;
                }
                this.draft.use_custom_editor = custom;
                if !custom {
                    this.draft.external_editor = name;
                } else if this.draft.custom_editor.is_none() {
                    this.draft.custom_editor = Some(corvene_core::CustomIntegration::default());
                }
                cx.notify();
            })
            .ok();
        });
        let weak = cx.weak_entity();
        let shell_names = shells.clone();
        let on_shell: SelectHandler = Rc::new(move |ix, _, cx| {
            let name = shell_names.get(ix).map(|n| n.to_string());
            let custom = ix == shell_names.len();
            weak.update(cx, |this, cx| {
                this.draft.use_custom_shell = custom;
                if !custom {
                    this.draft.shell = name;
                } else if this.draft.custom_shell.is_none() {
                    this.draft.custom_shell = Some(corvene_core::CustomIntegration::default());
                }
                cx.notify();
            })
            .ok();
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(labeled(
                mac_or("External Editor", "External editor"),
                select_button_items(
                    "prefs-editor",
                    editor_value,
                    with_app_icons(editor_options, editor_icons),
                    editor_ix,
                    false,
                    on_editor,
                    cx,
                ),
                cx,
            ))
            .when(editors.is_empty(), |d| {
                // `renderNoExternalEditorHint`
                d.child(
                    paragraph(vec![
                        "No other editors found. ".into(),
                        link_button("prefs-install-editor", "Install Visual Studio Code?", cx)
                            .on_click(|_, _, cx| {
                                Dispatcher::open_url(
                                    corvene_platform::editors::SUGGESTED_EDITOR_URL,
                                    cx,
                                )
                            })
                            .into_any_element()
                            .into(),
                    ])
                    .text_color(t.text_secondary),
                )
            })
            .when(use_custom_editor, |d| {
                let name = (editor_list
                    || s.flags.bool(corvene_core::flags::ids::CUSTOM_EDITOR_NAME))
                .then_some(&self.custom_editor_name);
                d.child(self.custom_form(
                    "custom-editor",
                    &self.custom_editor_path,
                    &self.custom_editor_args,
                    name,
                    window,
                    cx,
                ))
                // `523-custom-editor-list`
                .when(editor_list, |d| {
                    let weak = cx.weak_entity();
                    d.child(
                        div().flex().flex_row().child(
                            button(
                                "custom-editor-remove",
                                mac_or("Remove Custom Editor", "Remove custom editor"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                weak.update(cx, |this, cx| this.remove_custom_editor(window, cx))
                                    .ok();
                            }),
                        ),
                    )
                })
            })
            .child(labeled(
                "Shell",
                select_button_items(
                    "prefs-shell",
                    shell_value,
                    with_app_icons(shell_options, shell_icons),
                    shell_ix,
                    false,
                    on_shell,
                    cx,
                ),
                cx,
            ))
            .when(use_custom_shell, |d| {
                d.child(self.custom_form(
                    "custom-shell",
                    &self.custom_shell_path,
                    &self.custom_shell_args,
                    None,
                    window,
                    cx,
                ))
            })
            .when(android, |d| {
                d.child(android_shell_note(shells.is_empty(), cx))
            })
            // `350-ssh-key-helper` (always on Android, for its bundled ssh)
            .when(
                android || s.flags.bool(corvene_core::flags::ids::SSH_KEY_HELPER),
                |d| d.child(ssh_key_section(cx)),
            )
            .into_any_element()
    }

    fn git_tab(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let weak = cx.weak_entity();
        let tabs = tab_bar(
            vec![
                TabModel {
                    dot: false,
                    id: "prefs-git-author",
                    label: "Author".into(),
                    count: None,
                },
                TabModel {
                    dot: false,
                    id: "prefs-git-default-branch",
                    label: "Default branch".into(),
                    count: None,
                },
                TabModel {
                    dot: false,
                    id: "prefs-git-hooks",
                    label: "Hooks".into(),
                    count: None,
                },
            ],
            match self.git_tab {
                GitTab::Author => 0,
                GitTab::DefaultBranch => 1,
                GitTab::Hooks => 2,
            },
            move |ix, _, cx| {
                weak.update(cx, |this, cx| {
                    this.git_tab = match ix {
                        0 => GitTab::Author,
                        1 => GitTab::DefaultBranch,
                        _ => GitTab::Hooks,
                    };
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let edit_config = |cx: &App| {
            paragraph(vec![
                "These preferences will ".into(),
                link_button(
                    "prefs-edit-gitconfig",
                    "edit your global Git config file",
                    cx,
                )
                .text_size(FONT_SIZE_SM())
                .on_click(|_, _, cx| Dispatcher::edit_global_git_config(cx))
                .into_any_element()
                .into(),
                ".".into(),
            ])
            .mt(SPACING())
            .text_size(FONT_SIZE_SM())
            .text_color(t.text_secondary)
        };
        let body = match self.git_tab {
            GitTab::Hooks => div().into_any_element(),
            GitTab::Author => {
                let emails = self.account_emails(cx);
                let mut options: Vec<SharedString> = emails
                    .iter()
                    .map(|e| SharedString::from(e.clone()))
                    .collect();
                options.push(OTHER_EMAIL.into());
                let selected_ix = match &self.email_choice {
                    Some(choice) => emails.iter().position(|e| e == choice),
                    None => Some(emails.len()),
                };
                let weak = cx.weak_entity();
                let emails_for_select = emails.clone();
                let on_email: SelectHandler = Rc::new(move |ix, window, cx| {
                    let choice = emails_for_select.get(ix).cloned();
                    weak.update(cx, |this, cx| {
                        if let Some(email) = &choice {
                            this.email
                                .update(cx, |s, cx| s.set_value(email.clone(), window, cx));
                        }
                        this.email_choice = choice;
                        cx.notify();
                    })
                    .ok();
                });
                let email_value = self.email.read(cx).value().to_string();
                // `GitConfigUserForm`: `GitEmailNotFoundWarning` for every
                // account while "Other email" (the text box) is shown
                let email_is_other = emails.is_empty() || self.email_choice.is_none();
                let warning = email_is_other
                    .then(|| {
                        crate::git_email_not_found_warning::git_email_not_found_warning(
                            &self.state.read(cx).accounts,
                            &email_value,
                        )
                    })
                    .flatten();
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .child(labeled(
                        "Name",
                        text_box("prefs-git-name", &self.name, None, window, cx),
                        cx,
                    ))
                    .when(!emails.is_empty(), |d| {
                        d.child(labeled(
                            "Email",
                            select_button(
                                "prefs-git-email-select",
                                self.email_choice
                                    .clone()
                                    .unwrap_or_else(|| OTHER_EMAIL.to_string()),
                                options,
                                selected_ix,
                                false,
                                on_email,
                                cx,
                            ),
                            cx,
                        ))
                    })
                    .when(emails.is_empty() || self.email_choice.is_none(), |d| {
                        // `GitConfigUserForm` labels only the account-email
                        // select; flag `602-git-config-email-label` labels the
                        // lone text box too
                        let label_lone = self
                            .state
                            .read(cx)
                            .flags
                            .bool(corvene_core::flags::ids::GIT_CONFIG_EMAIL_LABEL);
                        d.child(if emails.is_empty() && label_lone {
                            labeled(
                                "Email",
                                text_box("prefs-git-email", &self.email, None, window, cx),
                                cx,
                            )
                            .into_any_element()
                        } else {
                            text_box("prefs-git-email", &self.email, None, window, cx)
                                .into_any_element()
                        })
                    })
                    .when_some(warning, |d, warning| {
                        d.child(
                            crate::git_email_not_found_warning::git_email_not_found_warning_element(
                                "prefs-email",
                                warning,
                                FONT_SIZE_SM(),
                                zpx(18.),
                                cx,
                            )
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary),
                        )
                    })
                    // `526-commit-signing`
                    .when(
                        self.state
                            .read(cx)
                            .flags
                            .bool(corvene_core::flags::ids::COMMIT_SIGNING),
                        |d| {
                            let sign = cx.weak_entity();
                            let ssh = cx.weak_entity();
                            d.child(div().mt(SPACING()).child(crate::widgets::signing_fields(
                                (
                                    "prefs-sign-commits",
                                    "prefs-signing-ssh",
                                    "prefs-signing-key",
                                ),
                                self.signing.sign,
                                self.signing.ssh,
                                &self.signing_key,
                                move |on, _, cx| {
                                    sign.update(cx, |this, cx| {
                                        this.signing.sign = on;
                                        cx.notify();
                                    })
                                    .ok();
                                },
                                move |on, _, cx| {
                                    ssh.update(cx, |this, cx| {
                                        this.signing.ssh = on;
                                        cx.notify();
                                    })
                                    .ok();
                                },
                                window,
                                cx,
                            )))
                        },
                    )
                    .child(edit_config(cx))
                    .into_any_element()
            }
            GitTab::DefaultBranch => {
                let value = self.default_branch.read(cx).value().to_string();
                let sanitized = crate::dialogs::branch_dialogs::sanitize_ref_name(value.trim());
                let quiet = self
                    .state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::BRANCH_NAME_TRAILING_SLASH_QUIET);
                let path_settings = self
                    .state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::PATH_GIT_SETTINGS);
                let warning = crate::dialogs::branch_dialogs::ref_name_warning(&value, "", quiet)
                    .then(|| format!("Will be saved as {sanitized}."));
                div()
                    .flex()
                    .flex_col()
                    .child(section_heading("Default branch name for new repositories", cx))
                    .child(text_box(
                        "prefs-default-branch",
                        &self.default_branch,
                        None,
                        window,
                        cx,
                    ))
                    .when_some(warning, |d, warning| {
                        d.child(
                            div()
                                .mt(SPACING_THIRD())
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(warning),
                        )
                    })
                    .child(
                        paragraph(vec![
                            "GitHub's default branch name is ".into(),
                            code_ref("main", cx).into_any_element().into(),
                            ". You may want to change it due to different workflows, or because your integrations still require the historical default branch name of ".into(),
                            code_ref("master", cx).into_any_element().into(),
                            ".".into(),
                        ])
                        .mt(SPACING())
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary),
                    )
                    .when(path_settings, |d| d.child(self.path_settings(cx)))
                    .child(edit_config(cx))
                    .into_any_element()
            }
        };
        let body = match self.git_tab {
            GitTab::Hooks => self.hooks_tab(cx),
            _ => body,
        };
        // `.dialog-content.git-preferences { padding: 0 }`: the sub tab bar is
        // flush with the tab container; its content gets the 20 px padding back.
        div()
            .m(zpx(-20.))
            .flex()
            .flex_col()
            .child(tabs)
            .child(div().p(SPACING_DOUBLE()).child(body))
            .into_any_element()
    }

    /// `renderHooksSettings`
    fn hooks_tab(&self, cx: &Context<Self>) -> AnyElement {
        let enabled = self.draft.enable_git_hook_env;
        div()
            .flex()
            .flex_col()
            .child(checkbox_row(
                "prefs-hook-env",
                enabled,
                "Load Git hook environment variables from shell",
                self.edit(cx, |s, v| s.enable_git_hook_env = v),
                cx,
            ))
            .child(settings_description(cx).child(
                "When enabled, Corvene will attempt to load environment variables from your shell when executing Git hooks. This is useful if your Git hooks depend on environment variables set in your shell configuration files, a common practice for version managers such as nvm, rbenv, asdf, etc.",
            ))
            .when(enabled, |d| {
                d.child(div().mt(SPACING()).child(checkbox_row(
                    "prefs-hook-env-cache",
                    self.draft.cache_git_hook_env,
                    "Cache Git hook environment variables",
                    self.edit(cx, |s, v| s.cache_git_hook_env = v),
                    cx,
                )))
                .child(settings_description(cx).child(
                    "Cache hook environment variables to improve performance. Disable if your hooks rely on frequently changing environment variables.",
                ))
            })
            .into_any_element()
    }

    /// `renderFormatting`: Date / Time / Number format + absolute dates.
    fn formatting_section(&self, cx: &Context<Self>) -> Div {
        let date_options: Vec<SharedString> = crate::format::DATE_FORMATS
            .iter()
            .map(|p| format!("{} ({p})", crate::format::date_example(p)).into())
            .collect();
        let date_ix = crate::format::DATE_FORMATS
            .iter()
            .position(|p| *p == self.draft.date_format);
        let weak = cx.weak_entity();
        let on_date: SelectHandler = Rc::new(move |ix, _, cx| {
            if let Some(p) = crate::format::DATE_FORMATS.get(ix) {
                weak.update(cx, |this, cx| {
                    this.draft.date_format = p.to_string();
                    cx.notify();
                })
                .ok();
            }
        });
        let time_options: Vec<SharedString> = crate::format::TIME_FORMATS
            .iter()
            .map(|p| format!("{} ({p})", crate::format::time_example(p)).into())
            .collect();
        let time_ix = crate::format::TIME_FORMATS
            .iter()
            .position(|p| *p == self.draft.time_format);
        let weak = cx.weak_entity();
        let on_time: SelectHandler = Rc::new(move |ix, _, cx| {
            if let Some(p) = crate::format::TIME_FORMATS.get(ix) {
                weak.update(cx, |this, cx| {
                    this.draft.time_format = p.to_string();
                    cx.notify();
                })
                .ok();
            }
        });
        let number_options: Vec<SharedString> = crate::format::NUMBER_FORMATS
            .iter()
            .map(|k| crate::format::number_example(k).into())
            .collect();
        let number_ix = crate::format::NUMBER_FORMATS
            .iter()
            .position(|k| *k == self.draft.number_format);
        let weak = cx.weak_entity();
        let on_number: SelectHandler = Rc::new(move |ix, _, cx| {
            if let Some(k) = crate::format::NUMBER_FORMATS.get(ix) {
                weak.update(cx, |this, cx| {
                    this.draft.number_format = k.to_string();
                    cx.notify();
                })
                .ok();
            }
        });
        let pick = |options: &[SharedString], ix: Option<usize>| {
            ix.and_then(|i| options.get(i).cloned()).unwrap_or_default()
        };
        div()
            .mt(SPACING())
            .flex()
            .flex_col()
            .child(section_heading("Formatting", cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(SPACING())
                    .mb(SPACING())
                    .child(
                        labeled(
                            mac_or("Date Format", "Date format"),
                            select_button(
                                "prefs-date-format",
                                pick(&date_options, date_ix),
                                date_options.clone(),
                                date_ix,
                                false,
                                on_date,
                                cx,
                            ),
                            cx,
                        )
                        // the selects size to their longest option: 207 px here,
                        // the time format takes the rest
                        .flex_none()
                        .w(zpx(207.)),
                    )
                    .child(labeled(
                        mac_or("Time Format", "Time format"),
                        select_button(
                            "prefs-time-format",
                            pick(&time_options, time_ix),
                            time_options.clone(),
                            time_ix,
                            false,
                            on_time,
                            cx,
                        ),
                        cx,
                    )),
            )
            .child(labeled(
                mac_or("Number Format", "Number format"),
                select_button(
                    "prefs-number-format",
                    pick(&number_options, number_ix),
                    number_options.clone(),
                    number_ix,
                    false,
                    on_number,
                    cx,
                ),
                cx,
            ))
            .child(div().my(SPACING()).child(checkbox_row(
                "prefs-absolute-dates",
                self.draft.prefer_absolute_dates,
                "Prefer absolute dates over relative",
                self.edit(cx, |s, v| s.prefer_absolute_dates = v),
                cx,
            )))
    }

    fn appearance_tab(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        // `101-high-contrast-theme` off: GHD's three swatches, and a saved
        // High Contrast shows as Dark (the setting itself is kept)
        let high_contrast = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::HIGH_CONTRAST_THEME);
        let themes: Vec<(ThemeSetting, &str)> = [
            (ThemeSetting::Light, "Light"),
            (ThemeSetting::Dark, "Dark"),
            (ThemeSetting::System, "System"),
            // Corvene addition (GHD has no high contrast theme)
            (ThemeSetting::HighContrast, "High Contrast"),
        ]
        .into_iter()
        .filter(|(theme, _)| high_contrast || *theme != ThemeSetting::HighContrast)
        .collect();
        let selected = corvene_core::flags::effective_theme(self.draft.theme, high_contrast);
        let swatches = div()
            .flex()
            .flex_row()
            .mx(zpx(-5.))
            .children(themes.iter().map(|(theme, label)| {
                let theme = *theme;
                let is_selected = theme == selected;
                let compact = theme == ThemeSetting::HighContrast;
                let weak = cx.weak_entity();
                let id: &'static str = match theme {
                    ThemeSetting::Light => "prefs-theme-light",
                    ThemeSetting::Dark => "prefs-theme-dark",
                    ThemeSetting::System => "prefs-theme-system",
                    ThemeSetting::HighContrast => "prefs-theme-high-contrast",
                };
                let image = |path: &'static str| {
                    img(path)
                        .w_full()
                        .h(zpx(60.))
                        .object_fit(ObjectFit::Cover)
                        .border_b_1()
                        .border_color(t.box_border)
                };
                div()
                    .id(id)
                    .flex_1()
                    .min_w_0()
                    .p(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.draft.theme = theme;
                            cx.notify();
                        })
                        .ok();
                    })
                    .child(
                        div()
                            .w_full()
                            // as tall as the two-line High Contrast swatch
                            .h_full()
                            .flex()
                            .flex_col()
                            .rounded(BORDER_RADIUS())
                            .border_1()
                            .border_color(if is_selected {
                                t.box_selected_active_background
                            } else {
                                t.box_border
                            })
                            .overflow_hidden()
                            .pb(SPACING_HALF())
                            .child(match theme {
                                ThemeSetting::Light => {
                                    image("illustrations/ghd_light.svg").into_any_element()
                                }
                                ThemeSetting::Dark => {
                                    image("illustrations/ghd_dark.svg").into_any_element()
                                }
                                ThemeSetting::HighContrast => {
                                    image("illustrations/ghd_high_contrast.svg").into_any_element()
                                }
                                // `.system-theme-swatch`: light on the left half, dark on the right.
                                ThemeSetting::System => div()
                                    .relative()
                                    .w_full()
                                    .h(zpx(60.))
                                    .border_b_1()
                                    .border_color(t.box_border)
                                    .overflow_hidden()
                                    .child(
                                        div().absolute().inset_0().child(
                                            img("illustrations/ghd_light.svg")
                                                .w_full()
                                                .h(zpx(60.))
                                                .object_fit(ObjectFit::Cover),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .right_0()
                                            .w_1_2()
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .absolute()
                                                    .top_0()
                                                    .bottom_0()
                                                    .right_0()
                                                    .w(zpx(230.))
                                                    .child(
                                                        img("illustrations/ghd_dark.svg")
                                                            .w_full()
                                                            .h(zpx(60.))
                                                            .object_fit(ObjectFit::Cover),
                                                    ),
                                            ),
                                    )
                                    .into_any_element(),
                            })
                            .child(
                                div()
                                    .mt(SPACING_HALF())
                                    // the fourth swatch's two-line label needs the room
                                    .px(if compact { zpx(3.) } else { SPACING() })
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(if compact { zpx(3.) } else { SPACING_HALF() })
                                    .child(radio(
                                        ElementId::from(SharedString::from(format!("{id}-radio"))),
                                        is_selected,
                                        cx,
                                    ))
                                    .child(if compact {
                                        div().min_w_0().line_height(zpx(14.)).child(*label)
                                    } else {
                                        div().whitespace_nowrap().child(*label)
                                    }),
                            ),
                    )
            }));
        let tab_size = self.draft.tab_size;
        let options: Vec<SharedString> = TAB_SIZES
            .iter()
            .map(|n| {
                if *n == TAB_SIZE_DEFAULT {
                    format!("{n} (default)").into()
                } else {
                    n.to_string().into()
                }
            })
            .collect();
        let selected_ix = TAB_SIZES.iter().position(|n| *n == tab_size);
        let tree_sitter = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::TREE_SITTER_HIGHLIGHTING);
        let language_extensions = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::LANGUAGE_EXTENSIONS);
        let weak = cx.weak_entity();
        let on_tab_size: SelectHandler = Rc::new(move |ix, _, cx| {
            if let Some(n) = TAB_SIZES.get(ix) {
                weak.update(cx, |this, cx| {
                    this.draft.tab_size = *n;
                    cx.notify();
                })
                .ok();
            }
        });
        div()
            .flex()
            .flex_col()
            .child(section_heading("Theme", cx))
            .child(swatches)
            .child(self.formatting_section(cx))
            // the absolute-dates checkbox's 10 px margin spaces the section;
            // GHD 3.6.7 `renderMiscellaneous` (was "Diff" with "Tab Size")
            .child(section_heading("Miscellaneous", cx))
            .child(labeled(
                mac_or("Diff Tab Size", "Diff tab size"),
                select_button(
                    "prefs-tab-size",
                    options
                        .get(selected_ix.unwrap_or(3))
                        .cloned()
                        .unwrap_or_default(),
                    options,
                    selected_ix,
                    false,
                    on_tab_size,
                    cx,
                ),
                cx,
            ))
            // `.always-show-worktree-list { margin-top: var(--spacing) }`
            .child(div().mt(SPACING()).child(checkbox_row(
                "prefs-always-show-worktree-list",
                self.draft.always_show_worktree_list,
                "Always show worktree list",
                self.edit(cx, |s, v| s.always_show_worktree_list = v),
                cx,
            )))
            // Corvene addition: `105-tree-sitter-highlighting`
            .when(tree_sitter, |d| d.child(self.syntax_highlighter_field(cx)))
            // Corvene addition: `111-language-extensions`
            .when(language_extensions, |d| {
                d.child(self.language_extensions_field(cx))
            })
            // Corvene addition: `117-file-icons`
            .when(
                self.state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::FILE_ICONS),
                |d| d.child(self.file_icons_field(language_extensions, cx)),
            )
            // Corvene addition: `1314-code-owners`
            .when(
                self.state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::CODE_OWNERS),
                |d| d.child(self.code_owners_field(cx)),
            )
            .into_any_element()
    }

    /// Appearance › Code owners (`1314-code-owners`): a file row's owners
    /// as a name or a shield icon.
    fn code_owners_field(&self, cx: &Context<Self>) -> AnyElement {
        use corvene_core::CodeOwnersDisplay;
        let choices = [CodeOwnersDisplay::Label, CodeOwnersDisplay::Icon];
        let options: Vec<SharedString> = vec!["Owner names".into(), "Shield icon".into()];
        let selected_ix = choices
            .iter()
            .position(|c| *c == self.draft.code_owners_display);
        let value = selected_ix
            .and_then(|ix| options.get(ix).cloned())
            .unwrap_or_else(|| options[0].clone());
        let weak = cx.weak_entity();
        let on_select: SelectHandler = Rc::new(move |ix, _, cx| {
            if let Some(choice) = choices.get(ix).copied() {
                weak.update(cx, |this, cx| {
                    this.draft.code_owners_display = choice;
                    cx.notify();
                })
                .ok();
            }
        });
        div()
            .flex()
            .flex_col()
            .mt(SPACING())
            .child(labeled(
                mac_or("Code Owners", "Code owners"),
                select_button(
                    "prefs-code-owners",
                    value,
                    options,
                    selected_ix,
                    false,
                    on_select,
                    cx,
                ),
                cx,
            ))
            .child(settings_description(cx).mt(zpx(2.)).child(
                "How files show their owners from the repository's CODEOWNERS file. Owners \
                 that are you or one of your teams are highlighted.",
            ))
            .into_any_element()
    }

    /// Appearance › File icons (`117-file-icons`): the built-in Octicons,
    /// none, or an installed extension's icon theme, and a way to find more.
    fn file_icons_field(&self, extensions: bool, cx: &Context<Self>) -> AnyElement {
        use corvene_core::file_icons::{BUILTIN, NONE, choices};
        let mut keys: Vec<String> = vec![BUILTIN.to_string(), NONE.to_string()];
        let mut options: Vec<SharedString> = vec!["Octicons".into(), "None".into()];
        let all = choices(self.state.read(cx));
        for choice in &all {
            // the same theme from VS Code and from Zed: say which
            let twin = all
                .iter()
                .filter(|c| c.label == choice.label && c.extension == choice.extension)
                .count()
                > 1;
            let label = match (choice.label == choice.extension, twin) {
                (true, false) => choice.label.clone(),
                (true, true) => format!("{} ({})", choice.label, choice.editor),
                (false, false) => format!("{} ({})", choice.label, choice.extension),
                (false, true) => {
                    format!("{} ({}, {})", choice.label, choice.extension, choice.editor)
                }
            };
            options.push(label.into());
            keys.push(choice.key.clone());
        }
        let selected_ix = keys.iter().position(|k| *k == self.draft.file_icon_theme);
        let value = selected_ix
            .and_then(|ix| options.get(ix).cloned())
            // a theme whose extension is gone or off
            .unwrap_or_else(|| "Octicons".into());
        let weak = cx.weak_entity();
        let on_select: SelectHandler = Rc::new(move |ix, _, cx| {
            if let Some(key) = keys.get(ix).cloned() {
                weak.update(cx, |this, cx| {
                    this.draft.file_icon_theme = key;
                    cx.notify();
                })
                .ok();
            }
        });
        div()
            .flex()
            .flex_col()
            .mt(SPACING())
            .child(labeled(
                mac_or("File Icons", "File icons"),
                select_button(
                    "prefs-file-icons",
                    value,
                    options,
                    selected_ix,
                    false,
                    on_select,
                    cx,
                ),
                cx,
            ))
            .child(settings_description(cx).mt(zpx(2.)).child(
                "The icon before each file in the changes and history lists. Icon themes \
                 come from VS Code and Zed extensions.",
            ))
            .when(extensions, |d| {
                d.child(div().mt(SPACING()).flex().flex_row().child(
                    button("prefs-find-icon-themes", "Find Icon Themes…", cx).on_click(
                        |_, _, cx| {
                            Dispatcher::open_language_extensions(
                                Some(corvene_core::extensions::ExtensionsFocus::IconThemes),
                                Some(corvene_core::PreferencesTab::Appearance),
                                cx,
                            )
                        },
                    ),
                ))
            })
            .into_any_element()
    }

    /// Appearance › Diff › Language extensions: the manager's entry point
    /// and how many extensions are installed.
    fn language_extensions_field(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let count = self.state.read(cx).extensions.installed.len();
        let installed = match count {
            0 => String::new(),
            1 => "1 installed".to_string(),
            n => format!("{n} installed"),
        };
        div()
            .flex()
            .flex_col()
            .mt(SPACING())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Language extensions"),
            )
            .child(settings_description(cx).mt(zpx(2.)).child(
                "Add highlighting for more languages with grammars from VS Code, Zed, Pulsar, \
                 Sublime Text or TextMate extensions.",
            ))
            .child(
                div()
                    .mt(SPACING())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .child(
                        button("prefs-language-extensions", "Language extensions…", cx).on_click(
                            |_, _, cx| {
                                Dispatcher::open_language_extensions(
                                    None,
                                    Some(corvene_core::PreferencesTab::Appearance),
                                    cx,
                                )
                            },
                        ),
                    )
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(installed),
                    ),
            )
            .into_any_element()
    }

    /// Appearance › Diff › Syntax highlighting: the engine and, when the
    /// choice needs grammars this build does not have, their download.
    fn syntax_highlighter_field(&self, cx: &Context<Self>) -> AnyElement {
        const CHOICES: [(SyntaxHighlighter, &str); 3] = [
            (SyntaxHighlighter::GitHubDesktop, "GitHub Desktop"),
            (
                SyntaxHighlighter::TreeSitterFallback,
                "Tree-sitter for other languages",
            ),
            (SyntaxHighlighter::TreeSitter, "Tree-sitter"),
        ];
        let t = cx.ghd();
        let current = self.draft.syntax_highlighter;
        let options: Vec<SharedString> = CHOICES.iter().map(|(_, l)| (*l).into()).collect();
        let selected_ix = CHOICES.iter().position(|(c, _)| *c == current);
        let weak = cx.weak_entity();
        let on_select: SelectHandler = Rc::new(move |ix, _, cx| {
            if let Some((choice, _)) = CHOICES.get(ix) {
                weak.update(cx, |this, cx| {
                    this.draft.syntax_highlighter = *choice;
                    cx.notify();
                })
                .ok();
            }
        });
        let description = match current {
            SyntaxHighlighter::GitHubDesktop => {
                "The CodeMirror modes GitHub Desktop uses; files in other languages \
                 get syntect's grammars."
            }
            SyntaxHighlighter::TreeSitterFallback => {
                "GitHub Desktop's colours where it highlights a language, tree-sitter for \
                 the others."
            }
            SyntaxHighlighter::TreeSitter => {
                "Tree-sitter for every language it has a grammar for, GitHub Desktop's \
                 highlighter for the rest."
            }
        };
        let packs = &self.state.read(cx).packs;
        let missing = packs.missing_for(current);
        let status = missing.map(|kind| {
            let (text, action) = self.pack_state(kind, "prefs-syntax-pack", cx);
            let text = format!("{}: {text}", kind.title());
            let error = packs.errors.get(&kind).map(|e| capitalize(e));
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .mt(SPACING())
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(SPACING())
                        .child(
                            div()
                                .id("prefs-syntax-pack-status")
                                .a11y_live(text.clone())
                                .flex_1()
                                .min_w_0()
                                .text_size(FONT_SIZE_SM())
                                .child(text),
                        )
                        .children(action.map(|a| div().flex_none().child(a))),
                )
                .children(error.map(|e| {
                    div()
                        .mt(SPACING_HALF())
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.error)
                        .child(e)
                }))
        });
        div()
            .flex()
            .flex_col()
            .mt(SPACING())
            .child(labeled(
                "Syntax highlighting",
                select_button(
                    "prefs-syntax-highlighter",
                    options
                        .get(selected_ix.unwrap_or(0))
                        .cloned()
                        .unwrap_or_default(),
                    options,
                    selected_ix,
                    false,
                    on_select,
                    cx,
                ),
                cx,
            ))
            .child(settings_description(cx).child(description))
            .children(status)
            .into_any_element()
    }

    fn notifications_tab(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let settings_url =
            corvene_platform::notifications::settings_url(corvene_platform::BUNDLE_ID);
        let permission = self
            .draft
            .notifications_enabled
            .then_some(self.notification_permission)
            .flatten();
        let notifications_link = |id: &'static str, cx: &Context<Self>| {
            let url = settings_url.clone();
            link_button(id, "Notifications Settings", cx)
                .on_click(move |_, _, cx| corvene_core::Dispatcher::open_url(&url, cx))
                .into_any_element()
        };
        // `renderNotificationHint`
        let mut parts: Vec<Inline> = vec![
            "Allows the display of notifications when high-signal events take place in the current repository."
                .into(),
        ];
        let mut warning: Option<Div> = None;
        match permission {
            None | Some(NotificationPermission::Unsupported) => {}
            Some(NotificationPermission::Default) => {
                parts.push(" You need to ".into());
                parts.push(Inline::Element(
                    link_button("prefs-notifications-grant", "grant permission", cx)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.grant_notification_permission(cx)),
                        )
                        .into_any_element(),
                ));
                parts.push(" to display these notifications from Corvene.".into());
            }
            Some(NotificationPermission::Denied) => {
                // `.setting-hint-warning`
                warning = Some(
                    div().mt(SPACING()).child(paragraph(vec![
                        Inline::Element(
                            crate::widgets::emoji("⚠️")
                                .text_color(t.dialog_warning)
                                .into_any_element(),
                        ),
                        " Corvene has no permission to display notifications. Please, enable them in the ".into(),
                        Inline::Element(notifications_link("prefs-notifications-settings", cx)),
                        ".".into(),
                    ])),
                );
            }
            Some(NotificationPermission::Granted) => {
                parts.push(
                    " Make sure notifications are properly configured for Corvene in the ".into(),
                );
                parts.push(Inline::Element(notifications_link(
                    "prefs-notifications-settings",
                    cx,
                )));
                parts.push(".".into());
            }
        }
        div()
            .flex()
            .flex_col()
            .child(section_heading("Notifications", cx))
            .child(checkbox_row(
                "prefs-notifications",
                self.draft.notifications_enabled,
                "Enable notifications",
                self.edit(cx, |s, v| s.notifications_enabled = v),
                cx,
            ))
            .child(
                settings_description(cx)
                    .child(paragraph(parts).line_height(zpx(16.)))
                    .children(warning),
            )
            .children(self.pull_request_events_section(cx))
            .into_any_element()
    }

    /// `354-pull-request-event-notifications`: which pull request events of
    /// any repository notify, and what a click opens.
    fn pull_request_events_section(&self, cx: &Context<Self>) -> Option<Div> {
        if !self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::PULL_REQUEST_EVENT_NOTIFICATIONS)
        {
            return None;
        }
        let d = &self.draft;
        let events = d.pull_request_event_notifications;
        let click = d.pull_request_notification_click;
        let clicks = [
            (
                corvene_core::NotificationClickAction::OpenInCorvene,
                "prefs-pr-events-click-corvene",
                "Open the repository in Corvene when it is in the list",
            ),
            (
                corvene_core::NotificationClickAction::OpenOnGitHub,
                "prefs-pr-events-click-github",
                "Open the pull request on GitHub",
            ),
        ];
        Some(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .mt(SPACING())
                        .child(section_heading("Pull request events", cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACING_HALF())
                        .child(checkbox_row(
                            "prefs-pr-events-review-requested",
                            events.review_requested,
                            "Review requested from you or your team",
                            self.edit(cx, |s, v| {
                                s.pull_request_event_notifications.review_requested = v
                            }),
                            cx,
                        ))
                        .child(checkbox_row(
                            "prefs-pr-events-reviews",
                            events.reviews,
                            "Your pull request approved or changes requested",
                            self.edit(cx, |s, v| s.pull_request_event_notifications.reviews = v),
                            cx,
                        ))
                        .child(checkbox_row(
                            "prefs-pr-events-merged",
                            events.merged,
                            "Your pull request merged",
                            self.edit(cx, |s, v| s.pull_request_event_notifications.merged = v),
                            cx,
                        ))
                        .child(checkbox_row(
                            "prefs-pr-events-mentions",
                            events.mentions,
                            "You or your team mentioned",
                            self.edit(cx, |s, v| s.pull_request_event_notifications.mentions = v),
                            cx,
                        )),
                )
                .child(settings_description(cx).child(paragraph(vec![
                    "Every signed-in account is checked for these events in any repository it can see, about once a minute."
                        .into(),
                ])))
                .child(
                    div()
                        .mt(SPACING())
                        .child(section_heading("When I click a notification...", cx)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACING_HALF())
                        .children(clicks.iter().map(|(value, id, label)| {
                            let value = *value;
                            let weak = cx.weak_entity();
                            radio_row(
                                id,
                                click == value,
                                *label,
                                move |_, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.draft.pull_request_notification_click = value;
                                        cx.notify();
                                    })
                                    .ok();
                                },
                                cx,
                            )
                        })),
                ),
        )
    }

    fn prompts_tab(&self, cx: &Context<Self>) -> AnyElement {
        let d = &self.draft;
        let copilot_prompt_omitted = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::COPILOT_PROMPT_OMITTED);
        let strategy = d.uncommitted_changes_strategy;
        let strategies = [
            (
                UncommittedChangesStrategy::AskForConfirmation,
                "prefs-strategy-ask",
                "Ask me where I want the changes to go",
            ),
            (
                UncommittedChangesStrategy::MoveToNewBranch,
                "prefs-strategy-move",
                "Always bring my changes to my new branch",
            ),
            (
                UncommittedChangesStrategy::StashOnCurrentBranch,
                "prefs-strategy-stash",
                "Always stash and leave my changes on the current branch",
            ),
        ];
        div()
            .flex()
            .flex_col()
            .child(section_heading("Show a confirmation dialog before...", cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_HALF())
                    .child(checkbox_row(
                        "prefs-confirm-remove",
                        d.confirm_repository_removal,
                        "Removing repositories",
                        self.edit(cx, |s, v| s.confirm_repository_removal = v),
                        cx,
                    ))
                    .child(checkbox_row(
                        "prefs-confirm-discard",
                        d.confirm_discard_changes,
                        "Discarding changes",
                        self.edit(cx, |s, v| s.confirm_discard_changes = v),
                        cx,
                    ))
                    .child(checkbox_row(
                        "prefs-confirm-discard-permanently",
                        d.confirm_discard_changes_permanently,
                        "Discarding changes permanently",
                        self.edit(cx, |s, v| s.confirm_discard_changes_permanently = v),
                        cx,
                    ))
                    .child(checkbox_row(
                        "prefs-confirm-discard-stash",
                        d.confirm_discard_stash,
                        "Discarding stash",
                        self.edit(cx, |s, v| s.confirm_discard_stash = v),
                        cx,
                    ))
                    .child(checkbox_row(
                        "prefs-confirm-checkout-commit",
                        d.confirm_checkout_commit,
                        "Checking out a commit",
                        self.edit(cx, |s, v| s.confirm_checkout_commit = v),
                        cx,
                    ))
                    .child(checkbox_row(
                        "prefs-confirm-force-push",
                        d.confirm_force_push,
                        "Force pushing",
                        self.edit(cx, |s, v| s.confirm_force_push = v),
                        cx,
                    ))
                    .child(checkbox_row(
                        "prefs-confirm-undo",
                        d.confirm_undo_commit,
                        "Undo commit",
                        self.edit(cx, |s, v| s.confirm_undo_commit = v),
                        cx,
                    ))
                    // `512-copilot-prompt-omitted`
                    .when(!copilot_prompt_omitted, |el| {
                        el.child(checkbox_row(
                            "prefs-confirm-commit-message-override",
                            d.confirm_commit_message_override,
                            "Overriding commit message with generated message",
                            self.edit(cx, |s, v| s.confirm_commit_message_override = v),
                            cx,
                        ))
                    })
                    .child(checkbox_row(
                        "prefs-confirm-worktree-removal",
                        d.confirm_worktree_removal,
                        "Removing worktrees",
                        self.edit(cx, |s, v| s.confirm_worktree_removal = v),
                        cx,
                    ))
                    .child(checkbox_row(
                        "prefs-confirm-filtered",
                        d.confirm_commit_filtered_changes,
                        "Committing changes hidden by filter",
                        self.edit(cx, |s, v| s.confirm_commit_filtered_changes = v),
                        cx,
                    )),
            )
            .child(div().mt(SPACING()).child(section_heading(
                "If I have changes and I switch branches...",
                cx,
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_HALF())
                    .children(strategies.iter().map(|(value, id, label)| {
                        let value = *value;
                        let weak = cx.weak_entity();
                        radio_row(
                            id,
                            strategy == value,
                            *label,
                            move |_, cx| {
                                weak.update(cx, |this, cx| {
                                    this.draft.uncommitted_changes_strategy = value;
                                    cx.notify();
                                })
                                .ok();
                            },
                            cx,
                        )
                    })),
            )
            .child(
                div()
                    .mt(SPACING())
                    .child(section_heading("Commit Length", cx)),
            )
            .child(checkbox_row(
                "prefs-commit-length",
                d.show_commit_length_warning,
                "Show commit length warning",
                self.edit(cx, |s, v| s.show_commit_length_warning = v),
                cx,
            ))
            .into_any_element()
    }

    /// Settings › Advanced › Menu bar (`428-menu-bar-status-item`): which
    /// repositories the status item watches, one checkbox each.
    fn menu_bar_section(&self, cx: &Context<Self>) -> Div {
        let t = cx.ghd();
        let repos: Vec<(u64, String, bool)> = self
            .state
            .read(cx)
            .repositories
            .iter()
            .filter(|r| !r.missing)
            .map(|r| {
                (
                    r.id,
                    r.name(),
                    self.draft.menu_bar_repositories.contains(&r.id),
                )
            })
            .collect();
        let weak = cx.weak_entity();
        div()
            .mt(SPACING())
            .flex()
            .flex_col()
            .child(section_heading("Menu bar", cx))
            .child(settings_description(cx).mb(SPACING()).child(
                "Show the ticked repositories in the menu bar: how far their branch is \
                         ahead of or behind its upstream, and the checks of what was pushed. The \
                         item's menu opens a repository or a failing check.",
            ))
            .when(repos.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child("No repositories added yet."),
                )
            })
            .children(repos.into_iter().map(|(id, name, watched)| {
                let weak = weak.clone();
                div()
                    .id(SharedString::from(format!("prefs-menu-bar-{id}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .mb(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.draft.menu_bar_repositories.retain(|&r| r != id);
                            if !watched {
                                this.draft.menu_bar_repositories.push(id);
                            }
                            cx.notify();
                        })
                        .ok();
                    })
                    .child(crate::widgets::checkbox(
                        ElementId::from(SharedString::from(format!("prefs-menu-bar-{id}-box"))),
                        watched,
                        false,
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(FONT_SIZE())
                            .child(name),
                    )
            }))
    }

    /// Settings › Advanced › Proxy (`528-proxy-credentials`): the proxy
    /// requests to GitHub.com go through and where it was found, and the
    /// saved proxy passwords, each with Forget.
    fn proxy_section(&self, cx: &Context<Self>) -> Div {
        let t = cx.ghd();
        let (status, logins) = &self.proxy_seen;
        let line = match status {
            None => "Looking up the proxy…".to_string(),
            Some(status) => match &status.proxy {
                None => "Requests to GitHub.com connect directly, without a proxy.".to_string(),
                Some(proxy) => format!(
                    "Requests to GitHub.com go through the proxy {proxy}, from {}.",
                    status.source
                ),
            },
        };
        let empty = logins.is_empty();
        div()
            .mt(SPACING())
            .flex()
            .flex_col()
            .child(section_heading("Proxy", cx))
            .child(settings_description(cx).mt_0().child(line))
            .child(settings_description(cx).mb(SPACING()).child(
                "When a proxy asks for a username and password, Corvene asks you once, \
                 keeps them in the keychain and uses them for Git and for its own requests.",
            ))
            .children(logins.iter().map(|(proxy, user)| {
                let id = SharedString::from(format!("prefs-proxy-forget-{proxy}"));
                let forget = proxy.clone();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .mb(SPACING_HALF())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(FONT_SIZE())
                            .child(format!("{user} at {proxy}")),
                    )
                    .child(button(id, "Forget", cx).on_click(move |_, _, cx| {
                        Dispatcher::forget_proxy_credentials(forget.clone(), cx)
                    }))
            }))
            .when(empty, |d| {
                d.child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child("No proxy passwords saved."),
                )
            })
    }

    fn advanced_tab(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let (crash_reports, offered_packs, clone_location, settings_file, menu_bar, proxy, stacked) = {
            use corvene_core::flags::ids;
            let flags = &self.state.read(cx).flags;
            (
                flags.bool(ids::CRASH_REPORTS),
                corvene_core::offered_packs(flags),
                flags.bool(ids::DEFAULT_CLONE_LOCATION),
                flags.bool(ids::SETTINGS_FILE),
                cfg!(target_os = "macos")
                    && (flags.bool(ids::MENU_BAR_STATUS_ITEM) || flags.bool(ids::MACOS_WIDGETS)),
                flags.bool(ids::PROXY_CREDENTIALS),
                flags.bool(ids::STACKED_DIFF),
            )
        };
        div()
            .flex()
            .flex_col()
            // Corvene (`1311-stacked-diff`): the tabs' stack buttons, as settings
            .when(stacked, |d| {
                d.child(section_heading("Diffs", cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(SPACING_HALF())
                            .child(checkbox_row(
                                "prefs-stacked-changes",
                                self.draft.stacked_diff_changes,
                                "Show the changes to be committed in one list (Changes)",
                                self.edit(cx, |s, v| s.stacked_diff_changes = v),
                                cx,
                            ))
                            .child(checkbox_row(
                                "prefs-stacked-history",
                                self.draft.stacked_diff_history,
                                "Show every file of a commit in one list (History)",
                                self.edit(cx, |s, v| s.stacked_diff_history = v),
                                cx,
                            )),
                    )
                    .child(div().mt(SPACING()))
            })
            .child(section_heading("Background updates", cx))
            .child(checkbox_row(
                "prefs-indicators",
                self.draft.repository_indicators_enabled,
                "Show status icons in the repository list",
                self.edit(cx, |s, v| s.repository_indicators_enabled = v),
                cx,
            ))
            .child(
                settings_description(cx)
                    .flex()
                    .flex_col()
                    .gap(SPACING())
                    .child(
                        "These icons indicate which repositories have local or remote changes, and require the periodic fetching of repositories that are not currently selected.",
                    )
                    .child(
                        "Turning this off will not stop the periodic fetching of your currently selected repository, but may improve overall app performance for users with many repositories.",
                    ),
            )
            .child(div().mt(SPACING()).child(section_heading("Network and credentials", cx)))
            .child(checkbox_row(
                "prefs-credential-manager",
                self.draft.use_external_credential_helper,
                "Use Git Credential Manager",
                self.edit(cx, |s, v| s.use_external_credential_helper = v),
                cx,
            ))
            .child(
                paragraph(vec![
                    "Use ".into(),
                    link_button("prefs-gcm-link", "Git Credential Manager", cx)
                        .text_size(FONT_SIZE_SM())
                        .on_click(|_, _, cx| Dispatcher::open_url("https://gh.io/gcm", cx))
                        .into_any_element()
                        .into(),
                    " for private repositories outside of GitHub.com. This feature is experimental and subject to change.".into(),
                ])
                .mt(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary),
            )
            // Corvene addition: `528-proxy-credentials`
            .when(proxy, |d| d.child(self.proxy_section(cx)))
            // Corvene addition: `428-menu-bar-status-item`
            .when(menu_bar, |d| d.child(self.menu_bar_section(cx)))
            // Corvene addition: `514-default-clone-location`
            .when(clone_location, |d| d.child(self.clone_location_section(cx)))
            // Corvene addition in place of GHD's Usage section (no telemetry);
            // `501-crash-reports`
            .when(crash_reports, |d| {
                d.child(div().mt(SPACING()).child(section_heading("Crash reports", cx)))
                    .child(checkbox_row(
                        "prefs-save-crash-reports",
                        self.draft.save_crash_reports,
                        "Save crash reports locally",
                        self.edit(cx, |s, v| s.save_crash_reports = v),
                        cx,
                    ))
                    .child(
                        settings_description(cx).child(CRASH_REPORTS_DESCRIPTION),
                    )
            })
            // Corvene addition: on-demand packs;
            // `105-tree-sitter-highlighting`
            .when(!offered_packs.is_empty(), |d| {
                d.child(div().mt(SPACING()).child(section_heading("Optional components", cx)))
                    .children(offered_packs.iter().map(|kind| self.pack_row(*kind, cx)))
            })
            .when(settings_file, |d| d.child(self.settings_file_section(cx)))
            .into_any_element()
    }

    /// Corvene (`522-settings-file`): where the settings file is, which
    /// settings it set, and "Export Settings…".
    fn settings_file_section(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let path = corvene_platform::paths::settings_file();
        let from_file: Vec<String> = self
            .state
            .read(cx)
            .settings_overlay
            .stored
            .keys()
            .cloned()
            .collect();
        let export_path = path.clone();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mt(SPACING())
                    .child(section_heading("Settings file", cx)),
            )
            .child(settings_description(cx).child(format!(
                "At launch Corvene applies the settings in {} over these; it never changes \
                 that file.",
                path.display()
            )))
            .when(!from_file.is_empty(), |d| {
                d.child(
                    settings_description(cx)
                        .text_color(t.text_secondary)
                        .child(format!("Set by the file: {}.", from_file.join(", "))),
                )
            })
            .child(
                div().mt(SPACING()).flex().flex_row().child(
                    button(
                        "prefs-export-settings",
                        mac_or("Export Settings…", "Export settings…"),
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        let directory = export_path
                            .parent()
                            .map(Path::to_path_buf)
                            .unwrap_or_default();
                        let receiver = cx.prompt_for_new_path(
                            &directory,
                            Some(corvene_core::settings_file::FILE_NAME),
                        );
                        cx.spawn(async move |cx| {
                            if let Ok(Ok(Some(path))) = receiver.await {
                                cx.update(|cx| Dispatcher::export_settings(path, cx));
                            }
                        })
                        .detach();
                    }),
                ),
            )
            .into_any_element()
    }

    /// `517-path-git-settings`: global `core.quotepath` (all platforms) and
    /// `core.longpaths` (Windows) under the default branch name.
    fn path_settings(&self, cx: &Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let set = move |apply: fn(&mut Self, bool)| {
            let weak = weak.clone();
            move |value: bool, _: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| {
                    apply(this, value);
                    cx.notify();
                })
                .ok();
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(div().mt(SPACING()).child(section_heading("Paths", cx)))
            .child(checkbox_row(
                "prefs-git-quotepath",
                !self.quotepath,
                "Show non-ASCII file names as they are (core.quotepath off)",
                set(|this, v| this.quotepath = !v),
                cx,
            ))
            .when(cfg!(windows), |d| {
                d.child(checkbox_row(
                    "prefs-git-longpaths",
                    self.longpaths,
                    "Allow paths longer than 260 characters (core.longpaths)",
                    set(|this, v| this.longpaths = v),
                    cx,
                ))
            })
            .into_any_element()
    }

    /// `514-default-clone-location`: where Clone, New Repository and the
    /// tutorial put repositories (`Settings::clone_dir`), with Choose… and a
    /// way back to GitHub Desktop's `~/Documents/GitHub`.
    fn clone_location_section(&self, cx: &Context<Self>) -> AnyElement {
        let chosen = self.draft.clone_dir.clone();
        let shown = chosen.clone().unwrap_or_else(|| {
            corvene_platform::paths::default_clone_dir_avoiding_onedrive(
                self.state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::CLONE_DIR_AVOIDS_ONEDRIVE),
            )
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mt(SPACING())
                    .child(section_heading("Clone location", cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .child(
                        div()
                            .id("prefs-clone-dir")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(shown.display().to_string()),
                    )
                    .child(
                        button("prefs-clone-dir-choose", "Choose…", cx)
                            .flex_none()
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.choose_clone_dir(window, cx)
                                }),
                            ),
                    ),
            )
            .child(
                settings_description(cx)
                    .mt(SPACING_HALF())
                    .child("Clone and New Repository suggest this folder for new repositories."),
            )
            .when(chosen.is_some(), |d| {
                d.child(
                    link_button("prefs-clone-dir-reset", "Use the default location", cx)
                        .mt(SPACING_HALF())
                        .text_size(FONT_SIZE_SM())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.draft.clone_dir = None;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    fn choose_clone_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    this.draft.clone_dir = Some(path);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// One on-demand pack: its state (compiled in / installed / downloading /
    /// not installed) and the Download / Remove action.
    fn pack_row(&self, kind: corvene_packs::PackKind, cx: &Context<Self>) -> AnyElement {
        use corvene_packs::PackKind;
        let t = cx.ghd();
        let packs = &self.state.read(cx).packs;
        let description = match kind {
            PackKind::TreeSitterAll => {
                "Tree-sitter grammars for every language, for Appearance › Syntax \
                 highlighting. Downloaded from Corvene's GitHub releases and verified \
                 before use."
            }
            PackKind::TreeSitterRest => {
                "Tree-sitter grammars for the languages GitHub Desktop does not \
                 highlight, enough for \"Tree-sitter for other languages\"."
            }
            PackKind::GitPortable => "A private copy of Git for machines without one.",
            PackKind::GitLfs => "Git Large File Storage for repositories that use it.",
            PackKind::LanguageExtensionsIndex => "Which extensions cover which file types.",
        };
        // Android's `play` flavour gets the grammars from Google Play
        let description = if corvene_packs::store_delivered(kind) {
            "Tree-sitter grammars for every language, for Appearance › Syntax \
             highlighting. Installed by Google Play."
        } else {
            description
        };
        let id = format!("prefs-pack-{}", kind.name());
        let (status, action) = self.pack_state(kind, &id, cx);
        let error = packs.errors.get(&kind).map(|e| capitalize(e));
        div()
            .flex()
            .flex_col()
            .mt(SPACING())
            .child(div().font_weight(FontWeight::SEMIBOLD).child(kind.title()))
            .child(settings_description(cx).mt(zpx(2.)).child(description))
            .child(
                div()
                    .mt(SPACING())
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(SPACING())
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}-status")))
                            .a11y_live(status.clone())
                            .flex_1()
                            .min_w_0()
                            .text_size(FONT_SIZE_SM())
                            .child(status),
                    )
                    .children(action.map(|a| div().flex_none().child(a))),
            )
            .children(error.map(|e| {
                div()
                    .mt(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.error)
                    .child(e)
            }))
            .into_any_element()
    }

    /// A pack's status line and its action (Download / Update + Remove),
    /// with element ids under `id`.
    fn pack_state(
        &self,
        kind: corvene_packs::PackKind,
        id: &str,
        cx: &Context<Self>,
    ) -> (String, Option<AnyElement>) {
        use corvene_packs::PackKind;
        let packs = &self.state.read(cx).packs;
        let entry = packs
            .manifest
            .as_ref()
            .and_then(|m| m.entry_for(kind, env!("CARGO_PKG_VERSION")).cloned());
        let size_mb = |bytes: u64| format!("{:.1} MB", bytes as f64 / 1_048_576.0);
        let element_id = |suffix: &str| SharedString::from(format!("{id}-{suffix}"));
        if packs.bundled(kind) {
            return ("Included in this build.".to_string(), None);
        }
        if let Some(progress) = packs.progress.get(&kind) {
            let text = match progress.total {
                Some(total) if total > 0 => format!(
                    "Downloading… {}%",
                    (progress.received * 100 / total).min(100)
                ),
                _ if packs.manifest_loading => "Checking what is available…".to_string(),
                _ => "Downloading…".to_string(),
            };
            return (text, None);
        }
        if let Some(installed) = packs.installed.get(&kind) {
            let newer = entry
                .as_ref()
                .filter(|e| e.version != installed.version)
                .map(|e| e.version.clone());
            let text = match newer {
                Some(v) => format!(
                    "Installed (version {}; {v} is available).",
                    installed.version
                ),
                None => format!("Installed (version {}).", installed.version),
            };
            let mut actions = div().flex().flex_row().items_center().gap(SPACING());
            if entry
                .as_ref()
                .is_some_and(|e| e.version != installed.version)
            {
                actions = actions.child(
                    button(element_id("update"), "Update", cx)
                        .on_click(move |_, _, cx| Dispatcher::install_pack(kind, cx)),
                );
            }
            actions = actions.child(
                button(element_id("remove"), "Remove", cx)
                    .on_click(move |_, _, cx| Dispatcher::uninstall_pack(kind, cx)),
            );
            return (text, Some(actions.into_any_element()));
        }
        if kind == PackKind::TreeSitterRest && packs.available(PackKind::TreeSitterAll) {
            return (
                "Not needed: the grammars for every language are installed.".to_string(),
                None,
            );
        }
        let label = match &entry {
            Some(e) if e.size > 0 => format!("Download ({})", size_mb(e.size)),
            _ => "Download".to_string(),
        };
        let text = match (&entry, &packs.manifest_error, packs.manifest_loading) {
            (Some(_), _, _) => "Not installed.".to_string(),
            (None, _, true) => "Not installed. Checking what is available…".to_string(),
            (None, Some(err), _) => format!("Not installed. {}", capitalize(err)),
            (None, None, _) if packs.manifest.is_some() => {
                "Not installed. No version for this Corvene is published.".to_string()
            }
            (None, None, _) => "Not installed.".to_string(),
        };
        let store = corvene_packs::store_delivered(kind);
        let enabled = entry.is_some() || store;
        let text = if store {
            "Not installed.".to_string()
        } else {
            text
        };
        (
            text,
            Some(
                button(element_id("download"), label, cx)
                    .when(!enabled, |d| d.opacity(0.6))
                    .on_click(move |_, _, cx| {
                        if enabled {
                            Dispatcher::install_pack(kind, cx);
                        }
                    })
                    .into_any_element(),
            ),
        )
    }

    fn accessibility_tab(&self, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let underline = self.draft.underline_links;
        div()
            .flex()
            .flex_col()
            .child(section_heading("Accessibility", cx))
            .child(checkbox_row(
                "prefs-underline-links",
                underline,
                "Underline links",
                self.edit(cx, |s, v| s.underline_links = v),
                cx,
            ))
            .child(
                paragraph(vec![
                    "When enabled, Corvene will underline links in commit messages, comments, and other text fields. This can help make links easier to distinguish. ".into(),
                    // `.example-link`: a non-interactive preview of the setting.
                    div()
                        .text_color(t.link)
                        .when(underline, |d| d.underline())
                        .child("This is an example link")
                        .into_any_element()
                        .into(),
                ])
                .mt(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary),
            )
            .child(div().mt(SPACING()).child(checkbox_row(
                "prefs-diff-check-marks",
                self.draft.show_diff_check_marks,
                "Show check marks in the diff",
                self.edit(cx, |s, v| s.show_diff_check_marks = v),
                cx,
            )))
            .child(settings_description(cx).child(
                "When enabled, check marks will be displayed along side the line numbers and groups of line numbers in the diff when committing. When disabled, the line number controls will be less prominent.",
            ))
            .into_any_element()
    }
}

impl Render for PreferencesDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let avatar_urls: Vec<String> = self
            .state
            .read(cx)
            .accounts
            .iter()
            .filter_map(|a| a.avatar_url.clone())
            .collect();
        for url in avatar_urls {
            Dispatcher::request_avatar_url(&url, cx);
        }
        let t = cx.ghd();
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let selected = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        let weak = cx.weak_entity();
        let compact = crate::theme::compact(window);
        let nav = crate::tab_bar::vertical_tab_bar_sized(
            vec![
                VerticalTab {
                    id: "prefs-tab-accounts",
                    label: "Accounts".into(),
                    icon: Octicon::Home,
                },
                VerticalTab {
                    id: "prefs-tab-integrations",
                    label: "Integrations".into(),
                    icon: Octicon::Person,
                },
                VerticalTab {
                    id: "prefs-tab-git",
                    label: "Git".into(),
                    icon: Octicon::GitCommit,
                },
                VerticalTab {
                    id: "prefs-tab-appearance",
                    label: "Appearance".into(),
                    icon: Octicon::Paintbrush,
                },
                VerticalTab {
                    id: "prefs-tab-notifications",
                    label: "Notifications".into(),
                    icon: Octicon::Bell,
                },
                VerticalTab {
                    id: "prefs-tab-prompts",
                    label: "Prompts".into(),
                    icon: Octicon::Question,
                },
                VerticalTab {
                    id: "prefs-tab-advanced",
                    label: "Advanced".into(),
                    icon: Octicon::Gear,
                },
                VerticalTab {
                    id: "prefs-tab-accessibility",
                    label: "Accessibility".into(),
                    icon: Octicon::Accessibility,
                },
            ],
            selected,
            compact,
            move |ix, _, cx| {
                weak.update(cx, |this, cx| {
                    this.tab = TABS[ix];
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        let body = match self.tab {
            PreferencesTab::Accounts => self.accounts_tab(window, cx),
            PreferencesTab::Integrations => self.integrations_tab(window, cx),
            PreferencesTab::Git => self.git_tab(window, cx),
            PreferencesTab::Appearance => self.appearance_tab(cx),
            PreferencesTab::Notifications => self.notifications_tab(cx),
            PreferencesTab::Prompts => self.prompts_tab(cx),
            PreferencesTab::Advanced => self.advanced_tab(cx),
            PreferencesTab::Accessibility => self.accessibility_tab(cx),
        };
        // `.preferences-container`: nav | bordered tab container. The dialog
        // content's 20 px padding is cancelled so the nav sits flush.
        // `renderErrors`: an invalid author name blocks saving (`gitAuthorNameIsValid`).
        let name_valid = corvene_core::git_author_name_is_valid(self.name.read(cx).value().trim());
        let error = (!name_valid).then_some(corvene_core::INVALID_GIT_AUTHOR_NAME_MESSAGE);
        // the 600 px dialog less its border
        let content = div()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.preferred_focus_visible {
                        this.preferred_focus_visible = false;
                        cx.notify();
                    }
                }),
            )
            .w(crate::theme::fit_bleed_width(598.))
            .mx(zpx(-20.))
            .my(zpx(-20.))
            .flex()
            .flex_col()
            .when_some(error, |d, message| {
                d.child(
                    crate::widgets::dialog_error_banner(message, cx)
                        .mx(zpx(0.))
                        .mt(zpx(0.))
                        .mb(zpx(0.)),
                )
            })
            .child(
                div()
                    // `dialog#preferences .dialog-content { min-height: 500px }`
                    // (`$default-tab-height`, 440 px before GHD 3.6.7)
                    .min_h(zpx(500.))
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .child(nav)
                    .child(
                        div()
                            .id("prefs-tab-container")
                            .flex_1()
                            .min_w_0()
                            .border_l_1()
                            .border_color(t.box_border)
                            .p(SPACING_DOUBLE())
                            // Advanced carries Corvene's extra sections (crash
                            // reports, optional components): it scrolls inside
                            // GHD's 500 px instead of growing the dialog
                            .when(self.tab == PreferencesTab::Advanced || compact, |d| {
                                d.max_h(zpx(500.)).overflow_y_scroll()
                            })
                            .child(body),
                    ),
            );
        let weak = cx.weak_entity();
        dialog(
            "dialog-preferences",
            corvene_platform::editors::SETTINGS_LABEL,
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "prefs-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "prefs-save",
                    label: "Save".into(),
                    disabled: !name_valid,
                    on_click: Box::new(move |_, cx| {
                        weak.update(cx, |this, cx| this.save(cx)).ok();
                    }),
                },
            }
            .into_buttons(),
            close,
            window,
            cx,
        )
    }
}

/// `#preferences .accounts-tab .call-to-action { display: block }`: the text
/// (10 px right margin), then the button 10 px below at its own width.
fn accounts_call_to_action(
    id: &'static str,
    body: &'static str,
    action_title: &'static str,
    focused: bool,
    on_action: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    div()
        .flex()
        .flex_col()
        .items_start()
        .child(div().self_stretch().mr(SPACING()).child(body))
        .child(
            div()
                .relative()
                .mt(SPACING())
                .when(focused, |d| d.child(crate::widgets::focus_ring(cx)))
                .child(
                    crate::widgets::primary_button(id, action_title, false, cx)
                        // `:focus` takes the hover background
                        .when(focused, |d| d.bg(t.button_hover_background))
                        .on_click(move |_, window, cx| on_action(window, cx)),
                ),
        )
}

/// Android, under the Shell select: Termux is the one terminal that opens in
/// a folder, and only after the user allowed it.
fn android_shell_note(missing: bool, cx: &App) -> AnyElement {
    const TERMUX_URL: &str = "https://f-droid.org/packages/com.termux/";
    let t = cx.ghd();
    let text: Vec<Inline> = if missing {
        vec![
            "Termux is not installed. ".into(),
            link_button("prefs-install-termux", "Install Termux?", cx)
                .on_click(|_, _, cx| Dispatcher::open_url(TERMUX_URL, cx))
                .into_any_element()
                .into(),
        ]
    } else {
        vec![
            "Termux and Corvene share repositories on shared storage (a folder under \
             /storage/emulated/0), not the ones in Corvene's own storage. Run the setup commands \
             in Termux once: they allow \"Open in Termux\", trust shared storage in Termux's git \
             and add two commands, corvene (opens a folder here) and corvene-git-config \
             (brings Termux's Git settings over; Corvene has a Git of its own and cannot \
             read Termux's). The editors Termux has (Neovim, Vim, Helix, Micro, nano, Emacs) \
             then show under External editor. If Termux stays in the background when \
             Corvene opens it, allow it \"Display over other apps\" in the system's settings."
                .into(),
        ]
    };
    let note = paragraph(text).text_color(t.text_secondary);
    if missing {
        return note.into_any_element();
    }
    #[cfg(target_os = "android")]
    let setup = corvene_platform::android::TERMUX_SETUP;
    #[cfg(not(target_os = "android"))]
    let setup = "";
    div()
        .flex()
        .flex_col()
        .gap(SPACING_HALF())
        .child(note)
        .child(div().flex().child(
            button("prefs-termux-setup", "Copy Termux setup commands", cx).on_click(
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(setup.into())),
            ),
        ))
        .into_any_element()
}

/// Android: the system's file picker, then `Dispatcher::import_ssh_key`.
#[cfg(target_os = "android")]
fn pick_ssh_key(window: &mut Window, cx: &mut App) {
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Import".into()),
    });
    window
        .spawn(cx, async move |cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                cx.update(|_, cx| Dispatcher::import_ssh_key(path, None, cx))
                    .ok();
            }
        })
        .detach();
}

/// Options › Integrations' SSH key: on Android the bundled ssh client's
/// (always), on the desktop the one in `~/.ssh` (`350-ssh-key-helper`).
fn ssh_key_section(cx: &App) -> AnyElement {
    const ADD_KEY_URL: &str = "https://github.com/settings/ssh/new";
    let android = cfg!(target_os = "android");
    let t = cx.ghd();
    let s = AppState::global(cx).read(cx);
    let helper = s.flags.bool(corvene_core::flags::ids::SSH_KEY_HELPER);
    // the accounts a key can be added to through the API, GitHub.com first
    let mut accounts: Vec<(String, String, String)> = s
        .accounts
        .iter()
        .filter(|_| helper)
        .map(|a| (a.key(), a.login.clone(), a.friendly_endpoint()))
        .collect();
    // `Account::key` is `endpoint|login` (`527-multiple-accounts`)
    accounts.sort_by_key(|(key, _, _)| {
        let endpoint = key.split_once('|').map_or(key.as_str(), |(e, _)| e);
        !corvene_github::Endpoint::from_api_base(endpoint).is_dotcom()
    });
    let busy = s.ssh_key.busy.clone();
    let status = s.ssh_key.status.clone();
    let section = div()
        .flex()
        .flex_col()
        .gap(SPACING_HALF())
        .mt(SPACING())
        .child(div().font_weight(FontWeight::SEMIBOLD).child("SSH key"));
    let note = match (&busy, &status) {
        (Some(text), _) => Some((text.clone(), t.text_secondary)),
        (None, Some((true, text))) => Some((text.clone(), t.text_secondary)),
        (None, Some((false, text))) => Some((text.clone(), t.dialog_error)),
        (None, None) => None,
    };
    let note = note.map(|(text, color)| {
        div()
            .text_size(FONT_SIZE_SM())
            .text_color(color)
            .child(text)
    });
    let waiting = busy.is_some();
    match corvene_platform::ssh_key::ssh_public_key() {
        Some(key) => {
            let copy = key.clone();
            let mut buttons = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(SPACING())
                .child(button("prefs-ssh-copy", "Copy public key", cx).on_click(
                    move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())),
                ));
            if !android {
                buttons = buttons.child(
                    button("prefs-ssh-agent", "Add to ssh-agent", cx)
                        .when(waiting, |b| b.opacity(0.6))
                        .on_click(move |_, _, cx| {
                            if !waiting {
                                Dispatcher::add_ssh_key_to_agent(cx)
                            }
                        }),
                );
            }
            if accounts.is_empty() {
                buttons = buttons.child(
                    link_button("prefs-ssh-add", "Add it to your GitHub account", cx)
                        .on_click(|_, _, cx| Dispatcher::open_url(ADD_KEY_URL, cx)),
                );
            } else {
                buttons = buttons.child(
                    button("prefs-ssh-upload", "Add to GitHub", cx)
                        .when(waiting, |b| b.opacity(0.6))
                        .on_click(move |ev, window, cx| {
                            if waiting {
                                return;
                            }
                            upload_ssh_key_to(&accounts, ev.position(), window, cx)
                        }),
                );
            }
            #[cfg(target_os = "android")]
            {
                buttons = buttons.child(
                    link_button("prefs-ssh-replace", "Use another key…", cx)
                        .on_click(|_, window, cx| pick_ssh_key(window, cx)),
                );
            }
            section
                .child(
                    div()
                        .id("prefs-ssh-key")
                        .p(SPACING_HALF())
                        .rounded(zpx(4.))
                        .border_1()
                        .border_color(t.box_border)
                        .font_family(crate::theme::mono_font())
                        .text_size(FONT_SIZE_SM())
                        .child(key),
                )
                .child(buttons)
                .children(note)
                .into_any_element()
        }
        None => {
            let buttons = div().flex().flex_row().flex_wrap().gap(SPACING()).child(
                button(
                    "prefs-ssh-create",
                    if android {
                        "Create SSH key"
                    } else {
                        mac_or("Create SSH Key…", "Create SSH key…")
                    },
                    cx,
                )
                .when(waiting, |b| b.opacity(0.6))
                .on_click(move |_, _, cx| {
                    if !waiting {
                        create_ssh_key_clicked(cx)
                    }
                }),
            );
            #[cfg(target_os = "android")]
            let buttons = buttons.child(
                button("prefs-ssh-import", "Import a key…", cx)
                    .on_click(|_, window, cx| pick_ssh_key(window, cx)),
            );
            section
                .child(div().text_color(t.text_secondary).child(if android {
                    "Remotes with an SSH address (git@…) need a key on this device."
                } else {
                    "Remotes with an SSH address (git@…) need a key. There is none in ~/.ssh yet."
                }))
                .child(buttons)
                .children(note)
                .into_any_element()
        }
    }
}

/// Create SSH key: Android makes one at once (no passphrase, no agent);
/// the desktop asks first (`Popup::CreateSshKey`).
fn create_ssh_key_clicked(cx: &mut App) {
    #[cfg(target_os = "android")]
    Dispatcher::create_ssh_key(cx);
    #[cfg(not(target_os = "android"))]
    Dispatcher::show_popup(Popup::CreateSshKey, cx);
}

/// Add to GitHub: the one account straight away, else a menu of them.
fn upload_ssh_key_to(
    accounts: &[(String, String, String)],
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = corvene_core::ssh_keys::default_key_title();
    if let [(endpoint, _, _)] = accounts {
        Dispatcher::upload_ssh_key(endpoint.clone(), title, cx);
        return;
    }
    let items = accounts
        .iter()
        .map(|(endpoint, login, host)| {
            let (endpoint, title) = (endpoint.clone(), title.clone());
            crate::context_menu::MenuItem::new(format!("{login} on {host}"), move |_, cx| {
                Dispatcher::upload_ssh_key(endpoint.clone(), title.clone(), cx)
            })
        })
        .collect();
    crate::native_menu::show_context_menu(items, position, window, cx);
}

/// Select options with the icon of the application each one stands for
/// (`icons`, in the options' order; the custom option has none).
fn with_app_icons(
    options: Vec<SharedString>,
    icons: Vec<Option<std::sync::Arc<Image>>>,
) -> Vec<SelectItem> {
    let mut icons = icons.into_iter();
    options
        .into_iter()
        .map(|label| SelectItem::IconOption(label, icons.next().flatten()))
        .collect()
}
