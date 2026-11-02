//! Banners under the toolbar - GHD `ui/banners/*.tsx`
//! (`styles/ui/_banners.scss`, `banners/_successful.scss`,
//! `banners/_conflicts.scss`): a 30 px strip with a green check (successes)
//! or an alert icon (conflicts), the message with bold branch names, an
//! optional "Undo" / "View conflicts" link and, when dismissable, an ✕.
//! `update_banner` is GHD's `UpdateAvailable` banner.
//!
//! [`BannerView`] is GHD's generic `Banner` (`ui/banners/banner.tsx`): 200 ms
//! after a banner appears focus moves to its first link (else its close
//! button), and the banner is dismissed `Banner::timeout()` after focus
//! leaves it (`corvene_core::banner_focus`).
//!
//! Deviation (`861-undo-delete-branch`): "Deleted branch" / "Restored
//! branch" banners, with an Undo that recreates the deleted branch, are
//! Corvene's (GHD deletes branches without a way back).
//!
//! Deviation (`322-git-email-mismatch-banner`): after signing in, a banner
//! says when the global Git email won't link commits to the account (GHD
//! warns only in Settings › Git and the commit form,
//! `ui/lib/git-email-not-found-warning.tsx`).
//!
//! Deviation (`287-repository-list-backup`): banners say when entries of
//! the stored repository list could not be read (and were set aside) or the
//! store could not be opened (GHD has neither case's message).
//!
//! Deviation (`294-follow-moved-repositories`): a banner names the new
//! location of a repository whose moved folder Corvene followed.
//!
//! Deviation (`422-banner-as-toast`): [`banner_toast_frame`] floats the
//! banner over the bottom-right corner instead of pushing the views down
//! (GHD `ui/app.tsx` `renderBanner` puts it in the layout flow).

use std::time::Instant;

use corvene_core::banner_focus::{BannerFocus, BannerFocusEvent};
use corvene_core::{
    AppState, AvailableUpdate, Banner, Dispatcher, PackageManager, Popup, PreferencesTab,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::widgets::{IconButtonA11y, ListRowA11y};

use crate::context_menu::mac_or;
use crate::icons::{Octicon, octicon};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::link_button;

#[allow(non_snake_case)]
pub fn BANNER_HEIGHT() -> Pixels {
    zpx(30.)
}

fn strong(text: impl Into<SharedString>) -> Div {
    div().font_weight(FontWeight::SEMIBOLD).child(text.into())
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "commit" } else { "commits" }
}

/// The message as text runs (`true` = bold, for branch names).
#[doc(hidden)]
pub fn parts(banner: &Banner) -> Vec<(String, bool)> {
    let t = |s: &str| (s.to_string(), false);
    let b = |s: &String| (s.clone(), true);
    match banner {
        Banner::SuccessfulMerge {
            our_branch,
            their_branch,
        } => match their_branch {
            Some(their) => vec![
                t("Successfully merged\u{a0}"),
                b(their),
                t("\u{a0}into\u{a0}"),
                b(our_branch),
            ],
            None => vec![t("Successfully merged into\u{a0}"), b(our_branch)],
        },
        Banner::SuccessfulRebase {
            target_branch,
            base_branch,
        } => match base_branch {
            Some(base) => vec![
                t("Successfully rebased\u{a0}"),
                b(target_branch),
                t("\u{a0}onto\u{a0}"),
                b(base),
            ],
            None => vec![t("Successfully rebased\u{a0}"), b(target_branch)],
        },
        Banner::BranchAlreadyUpToDate {
            our_branch,
            their_branch,
        } => match their_branch {
            Some(their) => vec![
                b(our_branch),
                t("\u{a0}is already up to date with\u{a0}"),
                b(their),
            ],
            None => vec![b(our_branch), t("\u{a0}is already up to date")],
        },
        Banner::SuccessfulCherryPick {
            target_branch,
            count,
            ..
        } => vec![
            (
                format!("Successfully copied {count} {} to\u{a0}", plural(*count)),
                false,
            ),
            b(target_branch),
            t("."),
        ],
        Banner::CherryPickUndone {
            target_branch,
            count,
        } => vec![
            (
                format!(
                    "Cherry-pick undone. Successfully removed the {count} copied {} from\u{a0}",
                    plural(*count)
                ),
                false,
            ),
            b(target_branch),
            t("."),
        ],
        Banner::SuccessfulSquash { count, .. } => vec![(
            format!("Successfully squashed {count} {}.", plural(*count)),
            false,
        )],
        Banner::FixupCommitted { target, .. } => {
            vec![t("Created a fixup commit for\u{a0}"), b(target)]
        }
        Banner::SquashUndone { count } => vec![(
            format!("Squash of {count} {} undone.", plural(*count)),
            false,
        )],
        Banner::SuccessfulReorder { count, .. } => vec![(
            format!("Successfully reordered {count} {}.", plural(*count)),
            false,
        )],
        Banner::ReorderUndone { count } => vec![(
            format!("Reorder of {count} {} undone.", plural(*count)),
            false,
        )],
        Banner::IssueCreated { number, .. } => {
            vec![t("Created issue\u{a0}"), b(&format!("#{number}"))]
        }
        Banner::ReviewSubmitted { number, event, .. } => {
            use corvene_core::pull_request_review::ReviewEvent;
            let verb = match event {
                ReviewEvent::Approve => "Approved pull request\u{a0}",
                ReviewEvent::RequestChanges => "Requested changes on pull request\u{a0}",
                ReviewEvent::Comment => "Reviewed pull request\u{a0}",
            };
            vec![t(verb), b(&format!("#{number}"))]
        }
        Banner::ReleaseCreated { name, draft, .. } => vec![
            t(if *draft {
                "Saved draft release\u{a0}"
            } else {
                "Created release\u{a0}"
            }),
            b(name),
        ],
        Banner::BranchDeleted { branch, .. } => vec![t("Deleted branch\u{a0}"), b(branch)],
        Banner::BranchRestored { branch } => vec![t("Restored branch\u{a0}"), b(branch)],
        Banner::BranchesDeleted { branches, .. } => {
            vec![(format!("Deleted {} branches", branches.len()), false)]
        }
        Banner::BranchesRestored { count } => {
            vec![(format!("Restored {count} branches"), false)]
        }
        Banner::StashDropped { message, .. } => vec![
            t("Discarded stash\u{a0}"),
            b(&corvene_core::stash_list::message_title(message)
                .unwrap_or_else(|| "stash".to_string())),
        ],
        Banner::StashRestored => vec![t("Restored stash")],
        Banner::SecurityKeyTouch { key, .. } => {
            vec![t(&format!("Touch your security key ({key}) to continue"))]
        }
        // `1315-discard-stash-file`
        Banner::StashFileDiscarded { path, .. } => vec![
            t("Discarded\u{a0}"),
            (path.rsplit('/').next().unwrap_or(path).to_string(), true),
            t("\u{a0}from the stash"),
        ],
        // `1106-apply-patch`
        Banner::PatchApplied {
            files,
            commits,
            conflicts,
        } => {
            let files_text = |n: usize| match n {
                1 => "1 file".to_string(),
                n => format!("{n} files"),
            };
            if *commits > 0 {
                vec![(
                    match commits {
                        1 => "Committed 1 patch".to_string(),
                        n => format!("Committed {n} patches"),
                    },
                    false,
                )]
            } else if *conflicts > 0 {
                vec![(
                    format!(
                        "Applied the patch. Resolve the conflicts in {} to finish.",
                        files_text(*conflicts)
                    ),
                    false,
                )]
            } else {
                vec![(
                    format!("Applied the patch to {}", files_text(*files)),
                    false,
                )]
            }
        }
        Banner::ConflictsFound {
            description,
            branch,
            ..
        } => match branch {
            Some(branch) => vec![
                (
                    format!("Resolve conflicts to continue {description}\u{a0}"),
                    false,
                ),
                b(branch),
                t("."),
            ],
            None => vec![(
                format!("Resolve conflicts to continue {description}."),
                false,
            )],
        },
        Banner::GitEmailMismatch { host, missing } => vec![(
            if *missing {
                format!(
                    "No Git email is set, so your commits won't be linked to your {host} account."
                )
            } else {
                format!(
                    "Your Git email isn't one of your {host} account's, so your commits won't be linked to it."
                )
            },
            false,
        )],
        Banner::RepositoriesUnreadable { raw: true, .. } => vec![t(
            "The repository list could not be read. Corvene kept a copy of it and started a new list.",
        )],
        Banner::RepositoriesUnreadable { count, .. } => vec![(
            if *count == 1 {
                "1 repository in the list could not be read and is set aside for a newer Corvene."
                    .to_string()
            } else {
                format!(
                    "{count} repositories in the list could not be read and are set aside for a newer Corvene."
                )
            },
            false,
        )],
        Banner::TemporaryStore { .. } => vec![t(
            "Corvene could not open its data, so nothing you change now is saved.",
        )],
        Banner::ConfigFileErrors { path, errors } => {
            let file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let first = errors.first().map(String::as_str).unwrap_or_default();
            let more = match errors.len() {
                0 | 1 => String::new(),
                n => format!(" (and {} more)", n - 1),
            };
            vec![
                t("Corvene could not use part of\u{a0}"),
                b(&file),
                (format!(": {first}{more}"), false),
            ]
        }
        Banner::RepositoryMoved { name, path } => vec![
            b(name),
            (
                format!("\u{a0}was moved; it now opens from {}", path.display()),
                false,
            ),
        ],
    }
}

/// The message line as a row of text runs (bold runs for branch names).
fn message(banner: &Banner) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .whitespace_nowrap()
        .children(parts(banner).into_iter().map(|(text, bold)| {
            if bold {
                strong(text).into_any_element()
            } else {
                div().child(text).into_any_element()
            }
        }))
}

/// The message as VoiceOver announces it.
fn plain_message(banner: &Banner) -> String {
    parts(banner)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<String>()
        .replace('\u{a0}', " ")
}

/// GHD `Banner.renderCloseButton`'s accessible name.
pub const DISMISS_LABEL: &str = "Dismiss this message";

/// GHD `Banner.renderCloseButton`: the close button's accessible name, `None`
/// for a banner that is not dismissable (no button).
pub fn close_button_label(dismissable: bool) -> Option<&'static str> {
    dismissable.then_some(DISMISS_LABEL)
}

/// GHD `Banner`: [`banner_bar`] for the app's banner, with its focus and
/// dismissal timers.
pub struct BannerView {
    state: Entity<AppState>,
    /// The banner's element (`this.banner`).
    container: FocusHandle,
    /// Its first suitable element: the first link, else the close button.
    first: FocusHandle,
    /// The shown banner's nonce and timers.
    focus: Option<(u64, BannerFocus)>,
    /// The banner drawn last, by nonce: focus only moves into a drawn one.
    drawn: Option<u64>,
    timer: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl BannerView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let container = cx.focus_handle();
        let first = cx.focus_handle();
        let subscriptions = vec![
            crate::windows::observe_state_in(&state, window, cx, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            // `onFocusIn`
            cx.on_focus_in(&container, window, |this, window, cx| {
                if let Some((_, focus)) = this.focus.as_mut() {
                    focus.focus_in();
                }
                this.schedule(window, cx);
            }),
            // `onFocusOut`: focus left the banner (moves inside it do not
            // leave it)
            cx.on_focus_out(&container, window, |this, _, window, cx| {
                if let Some((_, focus)) = this.focus.as_mut() {
                    focus.focus_out(Instant::now(), false);
                }
                this.schedule(window, cx);
            }),
        ];
        let mut this = Self {
            state,
            container,
            first,
            focus: None,
            drawn: None,
            timer: None,
            _subscriptions: subscriptions,
        };
        this.sync(window, cx);
        this
    }

    /// `componentDidMount` for a new banner; nothing pending without one.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let nonce = s.banner_nonce;
        match &s.banner {
            Some(banner) => {
                if self.focus.as_ref().map(|(n, _)| *n) != Some(nonce) {
                    let focus =
                        BannerFocus::mount(Instant::now(), banner.timeout(), banner.dismissable());
                    self.focus = Some((nonce, focus));
                    self.schedule(window, cx);
                }
            }
            None => {
                self.focus = None;
                self.timer = None;
            }
        }
    }

    /// Wait for the next due timer.
    fn schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(deadline) = self.focus.as_ref().and_then(|(_, f)| f.next_deadline()) else {
            self.timer = None;
            return;
        };
        let delay = deadline.saturating_duration_since(Instant::now());
        self.timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| this.fire(window, cx))
                .ok();
        }));
    }

    fn fire(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((nonce, focus)) = self.focus.as_mut() else {
            return;
        };
        let nonce = *nonce;
        for event in focus.advance(Instant::now()) {
            match event {
                // `focusOnFirstSuitableElement`
                BannerFocusEvent::FocusFirstElement => {
                    if self.drawn == Some(nonce) {
                        window.focus(&self.first, cx);
                    }
                }
                BannerFocusEvent::Dismiss => Dispatcher::dismiss_banner(nonce, cx),
            }
        }
        self.schedule(window, cx);
    }
}

impl Render for BannerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let (banner, nonce) = (s.banner.clone(), s.banner_nonce);
        self.drawn = banner.is_some().then_some(nonce);
        match banner {
            Some(banner) => {
                banner_bar(&banner, Some((&self.container, &self.first)), cx).into_any_element()
            }
            None => div().into_any_element(),
        }
    }
}

/// `renderBanner`: the banner strip; `focus`: the [`BannerView`]'s
/// container and first-suitable-element handles.
pub fn banner_bar(
    banner: &Banner,
    focus: Option<(&FocusHandle, &FocusHandle)>,
    cx: &App,
) -> impl IntoElement {
    let t = cx.ghd();
    let (container, first) = focus.map_or((None, None), |(c, f)| (Some(c), Some(f)));
    let is_conflicts = matches!(
        banner,
        Banner::ConflictsFound { .. }
            | Banner::PatchApplied { conflicts: 1.., .. }
            | Banner::GitEmailMismatch { .. }
            | Banner::RepositoriesUnreadable { .. }
            | Banner::TemporaryStore { .. }
            | Banner::ConfigFileErrors { .. }
    );
    let icon = if is_conflicts {
        octicon(Octicon::Alert, t.text).mr(SPACING())
    } else {
        octicon(Octicon::CheckCircleFill, t.color_new).mr(SPACING())
    };
    let action: Option<AnyElement> = match banner {
        // `894-cherry-pick-into-worktree-branch`: nothing to undo here
        Banner::SuccessfulCherryPick {
            undoable: false, ..
        } => None,
        Banner::SuccessfulCherryPick { repo, .. }
        | Banner::SuccessfulSquash { repo, .. }
        | Banner::SuccessfulReorder { repo, .. } => {
            let repo = *repo;
            Some(
                link_button("banner-undo", "Undo", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| {
                        Dispatcher::clear_banner(cx);
                        Dispatcher::undo_mco(repo, cx);
                    })
                    .into_any_element(),
            )
        }
        // Corvene (`799-fixup-commits`)
        Banner::FixupCommitted { repo, .. } => {
            let repo = *repo;
            Some(
                link_button("banner-squash-now", "Squash Now", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| {
                        Dispatcher::clear_banner(cx);
                        Dispatcher::autosquash(repo, false, cx);
                    })
                    .into_any_element(),
            )
        }
        // Corvene (`345-issues`, `346-releases`)
        Banner::IssueCreated { html_url, .. }
        | Banner::ReleaseCreated { html_url, .. }
        | Banner::ReviewSubmitted { html_url, .. } => {
            let url = html_url.clone();
            Some(
                link_button("banner-view-on-github", "View on GitHub", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| {
                        Dispatcher::clear_banner(cx);
                        Dispatcher::open_url(&url, cx);
                    })
                    .into_any_element(),
            )
        }
        Banner::BranchDeleted { repo, branch, sha } => {
            let (repo, branch, sha) = (*repo, branch.clone(), sha.clone());
            Some(
                link_button("banner-undo", "Undo", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| {
                        Dispatcher::clear_banner(cx);
                        Dispatcher::restore_deleted_branch(repo, branch.clone(), sha.clone(), cx);
                    })
                    .into_any_element(),
            )
        }
        Banner::StashDropped { repo, sha, message } => {
            let (repo, sha, message) = (*repo, sha.clone(), message.clone());
            Some(
                link_button("banner-undo", "Undo", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| {
                        Dispatcher::clear_banner(cx);
                        Dispatcher::restore_discarded_stash(repo, sha.clone(), message.clone(), cx);
                    })
                    .into_any_element(),
            )
        }
        Banner::StashFileDiscarded {
            repo,
            sha,
            message,
            position,
            replacing,
            ..
        } => {
            let (repo, sha, message, position, replacing) = (
                *repo,
                sha.clone(),
                message.clone(),
                *position,
                replacing.clone(),
            );
            Some(
                link_button("banner-undo", "Undo", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| {
                        Dispatcher::clear_banner(cx);
                        Dispatcher::undo_discard_stash_file(
                            repo,
                            sha.clone(),
                            message.clone(),
                            position,
                            replacing.clone(),
                            cx,
                        );
                    })
                    .into_any_element(),
            )
        }
        Banner::BranchesDeleted { repo, branches } => {
            let (repo, branches) = (*repo, branches.clone());
            Some(
                link_button("banner-undo", "Undo", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| {
                        Dispatcher::clear_banner(cx);
                        Dispatcher::restore_deleted_branches(repo, branches.clone(), cx);
                    })
                    .into_any_element(),
            )
        }
        Banner::ConflictsFound { repo, .. } => {
            let repo = *repo;
            Some(
                link_button("banner-view-conflicts", "View conflicts", cx)
                    .when_some(first, |d, first| d.track_focus(first))
                    .ml(SPACING_HALF())
                    .on_click(move |_, _, cx| Dispatcher::show_conflicts(repo, cx))
                    .into_any_element(),
            )
        }
        Banner::GitEmailMismatch { .. } => Some(
            link_button(
                "banner-git-settings",
                mac_or("Open Git Settings", "Open Git settings"),
                cx,
            )
            .when_some(first, |d, first| d.track_focus(first))
            .ml(SPACING_HALF())
            .on_click(|_, _, cx| {
                Dispatcher::clear_banner(cx);
                Dispatcher::show_popup(
                    Popup::Preferences {
                        tab: PreferencesTab::Git,
                    },
                    cx,
                );
            })
            .into_any_element(),
        ),
        Banner::TemporaryStore { path } | Banner::ConfigFileErrors { path, .. } => {
            let path = path.clone();
            Some(
                link_button(
                    "banner-reveal-store",
                    crate::context_menu::labels::REVEAL_IN_FILE_MANAGER,
                    cx,
                )
                .when_some(first, |d, first| d.track_focus(first))
                .ml(SPACING_HALF())
                .on_click(move |_, _, cx| Dispatcher::show_in_finder(&path, cx))
                .into_any_element(),
            )
        }
        _ => None,
    };
    let has_link = action.is_some();
    let close_color = t.text_secondary;
    let close_hover = t.text;
    div()
        .id("banner")
        .when_some(container, |d, container| d.track_focus(container))
        // announced when it appears (GHD renders banners in an aria-live region)
        .a11y_live(plain_message(banner))
        .w_full()
        .h(BANNER_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .pl(SPACING())
        .overflow_hidden()
        .bg(t.background)
        .border_b_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE())
        .text_color(t.text)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .child(icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(div().min_w_0().truncate().child(message(banner)))
                        .children(action),
                ),
        )
        .when_some(close_button_label(banner.dismissable()), |d, label| {
            d.child(
                div()
                    .id("banner-close")
                    // the first suitable element when there is no link
                    .when_some(first.filter(|_| !has_link), |d, first| d.track_focus(first))
                    .icon_button_label(label)
                    .mx(SPACING())
                    .flex_none()
                    .size(zpx(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_color(close_color)
                    .hover(move |s| s.text_color(close_hover))
                    .on_click(|_, _, cx| Dispatcher::clear_banner(cx))
                    .child(octicon(Octicon::X, close_color)),
            )
        })
}

/// `422-banner-as-toast`: a banner as a card in the window's bottom-right
/// corner, over the content. The banner's own bottom border is clipped (the
/// card has a full border).
pub fn banner_toast_frame(banner: impl IntoElement, cx: &App) -> impl IntoElement {
    let t = cx.ghd();
    div()
        .id("banner-toast")
        .occlude()
        .absolute()
        .right(SPACING_DOUBLE())
        .bottom(SPACING_DOUBLE())
        .w(zpx(480.))
        .max_w(relative(0.9))
        .rounded(BORDER_RADIUS())
        .border_1()
        .border_color(t.box_border)
        .bg(t.background)
        .overflow_hidden()
        .shadow(vec![BoxShadow {
            color: t.shadow,
            offset: point(zpx(0.), zpx(2.)),
            blur_radius: css_blur(7.),
            spread_radius: zpx(0.),
            inset: false,
        }])
        .child(div().mb(-zpx(1.)).child(banner))
}

/// GHD `ui/banners/update-available.tsx` (`#update-available`,
/// `banners/_update-available.scss`): a desktop-download icon in the warning
/// icon colour, "Corvene N is available", "what's new" opens the release
/// notes and "install and restart" installs (`updateNow`). A Homebrew
/// install is told to `brew upgrade corvene` instead (Linux: any other
/// package manager install is told to update with it). Always dismissable
/// (Corvene has no prioritised updates).
pub fn update_banner(
    update: &AvailableUpdate,
    manager: Option<PackageManager>,
    cx: &App,
) -> impl IntoElement {
    let t = cx.ghd();
    let version = update.version.clone();
    let plain = match manager {
        Some(PackageManager::System) => format!(
            "Corvene {version} is available. Update it with your package manager, or see what's new."
        ),
        Some(PackageManager::Homebrew) => format!(
            "Corvene {version} is available. Run brew upgrade corvene to install it, or see what's new."
        ),
        None => format!("Corvene {version} is available. See what's new or install and restart."),
    };
    let whats_new = link_button("update-banner-whats-new", "what's new", cx)
        .on_click(|_, _, cx| Dispatcher::show_update_release_notes(cx));
    let message = div()
        .flex()
        .flex_row()
        .items_center()
        .whitespace_nowrap()
        .child(format!("Corvene {version} is available.\u{a0}"))
        .map(|d| match manager {
            Some(PackageManager::System) => d
                .child("Update it with your package manager, or see\u{a0}")
                .child(whats_new)
                .child("."),
            Some(PackageManager::Homebrew) => d
                .child("Run\u{a0}")
                .child(
                    div()
                        .font_family(crate::theme::mono_font())
                        .child("brew upgrade corvene"),
                )
                .child("\u{a0}to install it, or see\u{a0}")
                .child(whats_new)
                .child("."),
            None => d
                .child("See\u{a0}")
                .child(whats_new)
                .child("\u{a0}or\u{a0}")
                .child(
                    link_button("update-banner-install", "install and restart", cx)
                        .on_click(|_, _, cx| Dispatcher::install_update(cx)),
                )
                .child("."),
        });
    let close_color = t.text_secondary;
    let close_hover = t.text;
    div()
        .id("update-available")
        .a11y_live(plain)
        .w_full()
        .h(BANNER_HEIGHT())
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .pl(SPACING())
        .overflow_hidden()
        .bg(t.background)
        .border_b_1()
        .border_color(t.box_border)
        .text_size(FONT_SIZE())
        .text_color(t.text)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .child(octicon(Octicon::DesktopDownload, t.banner_warning_icon).mr(SPACING()))
                .child(div().flex_1().min_w_0().truncate().child(message)),
        )
        .child(
            div()
                .id("update-banner-close")
                .icon_button_label(DISMISS_LABEL)
                .mx(SPACING())
                .flex_none()
                .size(zpx(16.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(close_color)
                .hover(move |s| s.text_color(close_hover))
                .on_click(|_, _, cx| Dispatcher::dismiss_update_banner(cx))
                .child(octicon(Octicon::X, close_color)),
        )
}
