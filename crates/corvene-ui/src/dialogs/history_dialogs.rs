//! History-operation dialogs: `ui/reset/warning-before-reset.tsx`,
//! `ui/checkout/confirm-checkout-commit.tsx`, `ui/create-tag/create-tag-dialog.tsx`,
//! `ui/undo/warn-local-changes-before-undo.tsx`.
//!
//! Deviation (`.docs/deviations.md` › History): Create a Tag has an
//! optional Message field (flag `823`); GHD always tags with an empty message.
//! Undoing a tagged commit warns first (flag `819`); ⌘⏎ submits Create a Tag
//! from its Message field (flag `824`). A pushed tag can be deleted, from the
//! remote too, after a confirmation (flag `826`). Create a Tag notes when
//! the account can only read the GitHub repository, so the tag cannot be
//! pushed there (flag `884-tag-push-permission-note`); GHD says nothing until
//! the push fails. Reset to Commit › Hard confirms in the reset-to-remote
//! dialog, naming the commits and changed files it discards (flag
//! `888-reset-modes`).

use corvene_core::{AppState, Dispatcher, UnreachableCommitsTab};
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{
    DialogButton, DialogKind, GroupButtonSpec, OkCancelButtonGroup, dialog, dialog_with_kind,
};
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{checkbox, text_box};

/// `WarningBeforeReset`
pub struct ResetToCommitDialog {
    repo: u64,
    sha: String,
}

impl ResetToCommitDialog {
    pub fn new(repo: u64, sha: String) -> Self {
        Self { repo, sha }
    }
}

impl Render for ResetToCommitDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, sha) = (self.repo, self.sha.clone());
        dialog_with_kind(
            "dialog-reset-to-commit",
            DialogKind::Warning,
            mac_or("Reset to Commit", "Reset to commit"),
            div().child(
                "You have changes in progress. Resetting to a previous commit might result in \
                 some of these changes being lost. Do you want to continue anyway?",
            ),
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "reset-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "reset-continue",
                    label: "Continue".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::reset_to_commit(repo, sha.clone(), cx);
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

/// Corvene addition (`261-reset-to-remote`): confirm resetting the current
/// branch to its upstream, or (`888-reset-modes`) hard-resetting it to a
/// commit.
pub struct ResetToRemoteDialog {
    repo: u64,
    branch: String,
    upstream: String,
    ahead: usize,
    dirty: bool,
    /// `888-reset-modes`: the commit to reset to, instead of the upstream.
    commit: Option<String>,
    /// `888-reset-modes`: the changed files the reset discards.
    files: Vec<String>,
}

impl ResetToRemoteDialog {
    pub fn new(repo: u64, branch: String, upstream: String, ahead: usize, dirty: bool) -> Self {
        Self {
            repo,
            branch,
            upstream,
            ahead,
            dirty,
            commit: None,
            files: Vec::new(),
        }
    }

    /// `888-reset-modes`: a Hard reset to `commit`, discarding `files`' changes.
    pub fn for_commit(mut self, commit: Option<String>, files: Vec<String>) -> Self {
        self.commit = commit;
        self.files = files;
        self
    }
}

/// `888-reset-modes`: "a.txt, b.txt and 3 more files" (the first few names).
fn name_files(files: &[String]) -> String {
    const SHOWN: usize = 3;
    let names: Vec<&str> = files
        .iter()
        .take(SHOWN)
        .map(|f| f.rsplit('/').next().unwrap_or(f))
        .collect();
    match files.len() {
        0 => String::new(),
        n if n <= SHOWN => match names.as_slice() {
            [only] => only.to_string(),
            [init @ .., last] => format!("{} and {last}", init.join(", ")),
            [] => String::new(),
        },
        n => format!(
            "{} and {} more {}",
            names.join(", "),
            n - SHOWN,
            if n - SHOWN == 1 { "file" } else { "files" }
        ),
    }
}

impl Render for ResetToRemoteDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, upstream, commit) = (self.repo, self.upstream.clone(), self.commit.clone());
        let mut lost = Vec::new();
        if self.ahead > 0 {
            lost.push(match (self.ahead, commit.is_some()) {
                (1, false) => "1 commit that is not on the remote".to_string(),
                (n, false) => format!("{n} commits that are not on the remote"),
                (1, true) => "1 commit".to_string(),
                (n, true) => format!("{n} commits"),
            });
        }
        if self.dirty {
            lost.push(if self.files.is_empty() {
                "all uncommitted changes".to_string()
            } else {
                format!("the uncommitted changes to {}", name_files(&self.files))
            });
        }
        let mut text = if commit.is_some() {
            format!("{} will be reset to commit {}.", self.branch, self.upstream)
        } else {
            format!("{} will be reset to match {}.", self.branch, self.upstream)
        };
        if !lost.is_empty() {
            text.push_str(&format!(
                " This discards {}. Do you want to continue?",
                lost.join(" and ")
            ));
        }
        let title = if commit.is_some() {
            mac_or("Hard Reset to Commit", "Hard reset to commit").to_string()
        } else {
            format!("Reset to {}", self.upstream)
        };
        dialog_with_kind(
            "dialog-reset-to-remote",
            DialogKind::Warning,
            title,
            div().child(text),
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "reset-remote-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "reset-remote-continue",
                    label: "Reset".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        match &commit {
                            Some(sha) => Dispatcher::reset_to_commit_with(
                                repo,
                                sha.clone(),
                                corvene_git::ResetMode::Hard,
                                cx,
                            ),
                            None => Dispatcher::reset_to_remote(repo, upstream.clone(), cx),
                        }
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

/// `ConfirmCheckoutCommit`
pub struct CheckoutCommitDialog {
    repo: u64,
    sha: String,
    dont_show_again: bool,
}

impl CheckoutCommitDialog {
    pub fn new(repo: u64, sha: String) -> Self {
        Self {
            repo,
            sha,
            dont_show_again: false,
        }
    }
}

impl Render for CheckoutCommitDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, sha, dont_show_again) = (self.repo, self.sha.clone(), self.dont_show_again);
        let content = div()
            .flex()
            .flex_col()
            .child(div().mb(SPACING()).child(
                "Checking out a commit will create a detached HEAD, and you will no longer be on \
                 any branch. Are you sure you want to checkout this commit?",
            ))
            .child(
                div()
                    .id("checkout-dont-show")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dont_show_again = !this.dont_show_again;
                        cx.notify();
                    }))
                    .child(checkbox(
                        "checkout-dont-show-box",
                        self.dont_show_again,
                        false,
                        cx,
                    ))
                    .child("Do not show this message again"),
            );
        dialog_with_kind(
            "dialog-checkout-commit",
            DialogKind::Warning,
            mac_or("Checkout Commit?", "Checkout commit?"),
            content,
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "checkout-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "checkout-ok",
                    label: "Checkout".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        if dont_show_again {
                            Dispatcher::update_settings(cx, |s| s.confirm_checkout_commit = false);
                        }
                        Dispatcher::close_popup(cx);
                        Dispatcher::checkout_commit(repo, sha.clone(), cx);
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

/// `CreateTag`
pub struct CreateTagDialog {
    repo: u64,
    sha: String,
    name: Entity<InputState>,
    /// Flag `823`: the annotated tag's message.
    message: Entity<TextareaState>,
}

/// GHD `MaxTagNameLength`
const MAX_TAG_NAME_LENGTH: usize = 245;

impl CreateTagDialog {
    pub fn new(repo: u64, sha: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        cx.observe(&name, |_, _, cx| cx.notify()).detach();
        // `RefNameTextBox` autoFocus
        let handle = name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        let message = cx.new(|cx| TextareaState::new(window, cx).rows(4));
        // ⏎ in Name submits the form, as GHD's `<form onSubmit>`; `824`: ⌘⏎
        // submits from the Message field too
        let cmd_enter = |cx: &App| {
            AppState::global(cx)
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::CMD_ENTER_SUBMITS_CREATE_TAG)
        };
        cx.subscribe(&name, move |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { secondary, .. } = ev
                && (!secondary || cmd_enter(cx))
            {
                this.submit(cx);
            }
        })
        .detach();
        cx.subscribe(&message, move |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
                && cmd_enter(cx)
            {
                this.submit(cx);
            }
        })
        .detach();
        Self {
            repo,
            sha,
            name,
            message,
        }
    }

    /// The trimmed name and its error (`getCurrentError`).
    fn name_and_error(&self, cx: &App) -> (String, Option<String>) {
        // GHD `RefNameTextBox` hands on `sanitizedRefName` of the input
        let name = crate::dialogs::branch_dialogs::sanitize_ref_name(&self.name.read(cx).value());
        let error = (name.len() > MAX_TAG_NAME_LENGTH).then(|| {
            format!("The tag name cannot be longer than {MAX_TAG_NAME_LENGTH} characters")
        });
        (name, error)
    }

    /// The message, empty unless flag `823` shows the Message field.
    fn message_text(&self, cx: &App) -> String {
        if AppState::global(cx)
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::TAG_MESSAGE)
        {
            self.message.read(cx).value().trim().to_string()
        } else {
            String::new()
        }
    }

    /// `createTag`
    fn submit(&mut self, cx: &mut Context<Self>) {
        let (name, error) = self.name_and_error(cx);
        if error.is_some() || name.is_empty() {
            return;
        }
        let message = self.message_text(cx);
        Dispatcher::close_popup(cx);
        Dispatcher::create_tag(self.repo, name, self.sha.clone(), message, cx);
    }
}

impl Render for CreateTagDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (name, error) = self.name_and_error(cx);
        let disabled = error.is_some() || name.is_empty();
        let this = cx.weak_entity();
        let (with_message, read_only_repo) = {
            let s = AppState::global(cx).read(cx);
            (
                s.flags.bool(corvene_core::flags::ids::TAG_MESSAGE),
                s.repository(self.repo)
                    .and_then(|r| r.github.as_ref())
                    .filter(|gh| {
                        !gh.has_write_permission()
                            && s.flags
                                .bool(corvene_core::flags::ids::TAG_PUSH_PERMISSION_NOTE)
                    })
                    .map(|gh| gh.full_name()),
            )
        };
        let t = cx.ghd();
        let content = crate::dialog::own_enter(div())
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .when_some(error, |d, message| {
                d.child(
                    div()
                        .mb(SPACING_HALF())
                        .text_color(t.form_error_text)
                        .child(message),
                )
            })
            .child(div().child("Name"))
            .child(text_box("tag-name", &self.name, None, window, cx))
            .children(crate::dialogs::branch_dialogs::ref_name_notice(
                &self.name.read(cx).value(),
                "created",
                cx,
            ))
            .when(with_message, |d| {
                d.child(div().mt(SPACING_HALF()).child("Message (optional)"))
                    .child(
                        div()
                            .border_1()
                            .border_color(t.box_border_contrast)
                            .rounded(BORDER_RADIUS())
                            .bg(t.box_background)
                            .overflow_hidden()
                            .child(crate::widgets::resizable_text_area(
                                "tag-message",
                                Textarea::new(&self.message),
                                cx,
                            )),
                    )
            })
            .when_some(read_only_repo, |d, repo| {
                d.child(
                    div()
                        .mt(SPACING_HALF())
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(format!(
                            "You can't push this tag to {repo}: your account can only read it. \
                             The tag will only exist on this computer."
                        )),
                )
            });
        dialog(
            "dialog-create-tag",
            mac_or("Create a Tag", "Create a tag"),
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "tag-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "tag-create",
                    label: mac_or("Create Tag", "Create tag").into(),
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        if !disabled {
                            this.update(cx, |this, cx| this.submit(cx)).ok();
                        }
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

/// `WarnLocalChangesBeforeUndo`
pub struct WarnLocalChangesBeforeUndoDialog {
    repo: u64,
    dont_show_again: bool,
}

impl WarnLocalChangesBeforeUndoDialog {
    pub fn new(repo: u64) -> Self {
        Self {
            repo,
            dont_show_again: false,
        }
    }
}

impl Render for WarnLocalChangesBeforeUndoDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, dont_show_again) = (self.repo, self.dont_show_again);
        let content = div()
            .flex()
            .flex_col()
            .child(div().mb(SPACING()).child(
                "You have changes in progress. Undoing the commit might result in some of these \
                 changes being lost. Do you want to continue anyway?",
            ))
            .child(
                div()
                    .id("undo-dont-show")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dont_show_again = !this.dont_show_again;
                        cx.notify();
                    }))
                    .child(checkbox(
                        "undo-dont-show-box",
                        self.dont_show_again,
                        false,
                        cx,
                    ))
                    .child("Do not show this message again"),
            );
        dialog_with_kind(
            "dialog-warn-undo",
            DialogKind::Warning,
            mac_or("Undo Commit", "Undo commit"),
            content,
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "undo-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "undo-continue",
                    label: "Continue".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        if dont_show_again {
                            Dispatcher::update_settings(cx, |s| s.confirm_undo_commit = false);
                        }
                        Dispatcher::close_popup(cx);
                        Dispatcher::undo_commit(repo, cx);
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

/// Flag `819`: the commit being undone or amended carries tags, which would
/// be left on a commit no branch contains (Corvene addition; GHD undoes and
/// amends silently).
pub struct WarnTaggedCommitBeforeUndoDialog {
    repo: u64,
    tags: Vec<String>,
    warn_local: bool,
    /// Amend Commit… on this commit instead of Undo.
    amend: Option<String>,
}

impl WarnTaggedCommitBeforeUndoDialog {
    pub fn new(repo: u64, tags: Vec<String>, warn_local: bool) -> Self {
        Self {
            repo,
            tags,
            warn_local,
            amend: None,
        }
    }

    pub fn amend(repo: u64, tags: Vec<String>, sha: String) -> Self {
        Self {
            repo,
            tags,
            warn_local: false,
            amend: Some(sha),
        }
    }
}

impl Render for WarnTaggedCommitBeforeUndoDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, warn_local, amend) = (self.repo, self.warn_local, self.amend.clone());
        let (noun, pronoun) = if self.tags.len() == 1 {
            ("tag", "It stays")
        } else {
            ("tags", "They stay")
        };
        let text = format!(
            "This commit has the {noun} {}. {pronoun} on the commit after it is {}, and \
             that commit will no longer be on any branch. Do you want to continue anyway?",
            self.tags
                .iter()
                .map(|t| format!("\u{201c}{t}\u{201d}"))
                .collect::<Vec<_>>()
                .join(", "),
            if amend.is_some() { "amended" } else { "undone" },
        );
        dialog_with_kind(
            "dialog-warn-undo-tagged",
            DialogKind::Warning,
            if amend.is_some() {
                mac_or("Amend Commit", "Amend commit")
            } else {
                mac_or("Undo Commit", "Undo commit")
            },
            div().child(text),
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "undo-tagged-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "undo-tagged-continue",
                    label: "Continue".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        if let Some(sha) = amend.clone() {
                            Dispatcher::start_amending(repo, sha, cx);
                        } else if warn_local {
                            Dispatcher::request_undo_commit_after_tags(repo, cx);
                        } else {
                            Dispatcher::undo_commit(repo, cx);
                        }
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

/// Flag `826`: delete a tag Corvene did not create-and-hold (not in
/// `tagsToPush`), optionally from the remote too (Corvene addition; GHD only
/// deletes unpushed tags). The remote box starts unticked. With
/// `remote_only` (`1219-tag-manager`'s Delete from Remote…) the tag goes
/// from the remote alone and stays here.
pub struct ConfirmDeletePushedTagDialog {
    repo: u64,
    tag: String,
    remote: Option<corvene_core::Remote>,
    from_remote: bool,
    remote_only: bool,
}

impl ConfirmDeletePushedTagDialog {
    pub fn new(repo: u64, tag: String, remote_only: bool, cx: &App) -> Self {
        Self {
            repo,
            tag,
            remote: Dispatcher::current_remote(repo, cx),
            from_remote: false,
            remote_only,
        }
    }
}

impl Render for ConfirmDeletePushedTagDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, tag) = (self.repo, self.tag.clone());
        let remote_only = self.remote_only;
        let remote = self
            .remote
            .clone()
            .filter(|_| self.from_remote || remote_only);
        let question = match (&self.remote, remote_only) {
            (Some(r), true) => format!(
                "Are you sure you want to delete the tag {} from {}? It stays in this repository.",
                self.tag, r.name
            ),
            _ => format!("Are you sure you want to delete the tag {}?", self.tag),
        };
        let content = div()
            .flex()
            .flex_col()
            .child(div().mb(SPACING()).child(question))
            .when_some(self.remote.clone().filter(|_| !remote_only), |d, r| {
                d.child(
                    div()
                        .id("delete-tag-remote")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.from_remote = !this.from_remote;
                            cx.notify();
                        }))
                        .child(checkbox(
                            "delete-tag-remote-box",
                            self.from_remote,
                            false,
                            cx,
                        ))
                        .child(format!("Also delete it from {}", r.name)),
                )
            });
        dialog_with_kind(
            "dialog-delete-pushed-tag",
            DialogKind::Warning,
            mac_or("Delete Tag", "Delete tag"),
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "delete-tag-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "delete-tag-confirm",
                    label: crate::dialog::confirm_label("Delete", "Delete Tag", "Delete tag", cx),
                    disabled: remote_only && remote.is_none(),
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        match (remote.clone(), remote_only) {
                            (Some(remote), true) => {
                                Dispatcher::delete_remote_tag_only(repo, tag.clone(), remote, cx)
                            }
                            (remote, _) => {
                                Dispatcher::delete_pushed_tag(repo, tag.clone(), remote, cx)
                            }
                        }
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

/// `ConfirmDiscardStash`; with an entry, the stash list's Discard
/// (`797-stash-list`).
pub struct ConfirmDiscardStashDialog {
    repo: u64,
    entry: Option<corvene_core::StashEntry>,
    dont_show_again: bool,
}

impl ConfirmDiscardStashDialog {
    pub fn new(repo: u64) -> Self {
        Self {
            repo,
            entry: None,
            dont_show_again: false,
        }
    }

    pub fn for_entry(repo: u64, entry: corvene_core::StashEntry) -> Self {
        Self {
            repo,
            entry: Some(entry),
            dont_show_again: false,
        }
    }
}

impl Render for ConfirmDiscardStashDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, dont_show_again) = (self.repo, self.dont_show_again);
        let entry = self.entry.clone();
        let content = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(SPACING())
                    .child("Are you sure you want to discard these stashed changes?"),
            )
            .child(
                div()
                    .id("discard-stash-dont-show")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dont_show_again = !this.dont_show_again;
                        cx.notify();
                    }))
                    .child(checkbox(
                        "discard-stash-dont-show-box",
                        self.dont_show_again,
                        false,
                        cx,
                    ))
                    .child("Do not show this message again"),
            );
        dialog_with_kind(
            "dialog-discard-stash",
            DialogKind::Warning,
            mac_or("Discard Stash?", "Discard stash?"),
            content,
            OkCancelButtonGroup {
                destructive: true,
                cancel: GroupButtonSpec {
                    id: "discard-stash-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "discard-stash-ok",
                    label: "Discard".into(),
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        if dont_show_again {
                            Dispatcher::update_settings(cx, |s| s.confirm_discard_stash = false);
                        }
                        Dispatcher::close_popup(cx);
                        match &entry {
                            Some(entry) => Dispatcher::discard_stash_entry(repo, entry.clone(), cx),
                            None => Dispatcher::drop_stash(repo, cx),
                        }
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

/// `UnreachableCommitsDialog` ("Commit Reachability"): which of the selected
/// commits the range diff includes (Reachable) and which it cannot
/// (Unreachable), with the commits listed like history rows.
pub struct UnreachableCommitsDialog {
    state: Entity<AppState>,
    repo: u64,
    tab: UnreachableCommitsTab,
}

impl UnreachableCommitsDialog {
    pub fn new(state: Entity<AppState>, repo: u64, tab: UnreachableCommitsTab) -> Self {
        Self { state, repo, tab }
    }
}

impl Render for UnreachableCommitsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (commits, is_unreachable) = {
            let s = self.state.read(cx);
            let rs = s.repo_states.get(&self.repo);
            let unreachable = self.tab == UnreachableCommitsTab::Unreachable;
            let commits: Vec<corvene_core::Commit> = rs
                .map(|rs| {
                    rs.selected_commits
                        .iter()
                        .filter(|sha| rs.shas_in_diff.contains(sha) != unreachable)
                        .filter_map(|sha| rs.commits.iter().find(|c| &c.sha == sha).cloned())
                        .collect()
                })
                .unwrap_or_default();
            (commits, unreachable)
        };
        let count = commits.len();
        let not = if is_unreachable { "not " } else { "" };
        let message = crate::widgets::paragraph(vec![
            format!(
                "You will {not}see changes from the following {} because {} {not}in the ancestry path of the most recent commit in your selection. ",
                if count > 1 { "commits" } else { "commit" },
                if count > 1 { "they're" } else { "it's" }
            )
            .into(),
            crate::widgets::link_button("unreachable-learn-more", "Learn more about unreachable commits.", cx)
                .on_click(|_, _, cx| {
                    Dispatcher::open_url(
                        "https://github.com/desktop/desktop/blob/development/docs/learn-more/unreachable-commits.md",
                        cx,
                    )
                })
                .into_any_element()
                .into(),
        ]);
        let weak = cx.weak_entity();
        let tabs = crate::tab_bar::tab_bar(
            vec![
                crate::tab_bar::TabModel {
                    dot: false,
                    id: "unreachable-tab-unreachable",
                    label: "Unreachable".into(),
                    count: None,
                },
                crate::tab_bar::TabModel {
                    dot: false,
                    id: "unreachable-tab-reachable",
                    label: "Reachable".into(),
                    count: None,
                },
            ],
            if is_unreachable { 0 } else { 1 },
            move |ix, _, cx| {
                weak.update(cx, |this, cx| {
                    this.tab = if ix == 0 {
                        UnreachableCommitsTab::Unreachable
                    } else {
                        UnreachableCommitsTab::Reachable
                    };
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
        // `.unreachable-commits { max-width: 400px }`, list ≥ 160 px
        let content = div()
            .w(crate::theme::fit_width(400.))
            .mx(zpx(-20.))
            .my(zpx(-20.))
            .flex()
            .flex_col()
            .child(tabs)
            .child(
                div()
                    .p(SPACING())
                    .border_b_1()
                    .border_color(t.box_border)
                    .text_size(FONT_SIZE())
                    .child(message),
            )
            .child(
                div()
                    .id("unreachable-commit-list")
                    .min_h(zpx(160.))
                    .max_h(zpx(300.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(commits.iter().map(|commit| {
                        div()
                            .id(SharedString::from(commit.sha.clone()))
                            .h(zpx(50.))
                            .flex_none()
                            .border_b_1()
                            .border_color(t.box_border)
                            .child(crate::history::commit_row_contents(
                                commit,
                                t.text,
                                t.text_secondary,
                                None,
                                cx,
                            ))
                    }))
                    .with_scrollbar(),
            );
        dialog(
            "dialog-unreachable-commits",
            mac_or("Commit Reachability", "Commit reachability"),
            content,
            vec![DialogButton {
                id: "unreachable-ok",
                label: "OK".into(),
                primary: true,
                disabled: false,
                on_click: Box::new(close),
            }],
            close,
            window,
            cx,
        )
    }
}
