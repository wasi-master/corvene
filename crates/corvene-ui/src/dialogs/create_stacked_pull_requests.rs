//! Corvene `356-stacked-pull-requests`: Branch › Create Stacked Pull
//! Requests… lists the branches from the default branch to HEAD, each with
//! the base its pull request gets. GHD has no such dialog; it is styled like
//! its `create-branch` dialog's rows and checkbox
//! (`ui/create-branch/create-branch-dialog.tsx`).

use corvene_core::{AppState, Dispatcher};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{DialogKind, GroupButtonSpec, OkCancelButtonGroup, dialog_with_kind};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{checkbox, code_ref};

/// `Popup::CreateStackedPullRequests`
pub struct CreateStackedPullRequestsDialog {
    state: Entity<AppState>,
    repo: u64,
    draft: bool,
    /// Create was pressed: pushing and asking GitHub.
    working: bool,
}

impl CreateStackedPullRequestsDialog {
    pub fn new(state: Entity<AppState>, repo: u64) -> Self {
        Self {
            state,
            repo,
            draft: false,
            working: false,
        }
    }
}

impl Render for CreateStackedPullRequestsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let t = cx.ghd().clone();
        let (repo, draft, working) = (self.repo, self.draft, self.working);
        let branches = self.state.read(cx).stack_branches(repo);
        let rows = branches.iter().enumerate().map(|(i, branch)| {
            div()
                .id(("stacked-pull-request", i))
                .flex()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(SPACING_HALF())
                .child(code_ref(branch.name.clone(), cx))
                .child(
                    div()
                        .text_color(t.text_secondary)
                        .child(format!("into {}", branch.base)),
                )
                .when_some(branch.pull_request, |d, number| {
                    d.child(
                        div()
                            .text_color(t.text_secondary)
                            .child(format!("(#{number})")),
                    )
                })
        });
        let content = div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                "Each branch is published and gets a pull request on the branch below it. A \
                 pull request that already exists keeps its title and gets this base.",
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_HALF())
                    .children(rows.rev()),
            )
            .child(
                div()
                    .id("stacked-pull-requests-draft")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.draft = !this.draft;
                        cx.notify();
                    }))
                    .child(checkbox(
                        "stacked-pull-requests-draft-box",
                        draft,
                        false,
                        cx,
                    ))
                    .child("Create as drafts"),
            );
        let this = cx.entity().downgrade();
        dialog_with_kind(
            "dialog-create-stacked-pull-requests",
            DialogKind::Normal,
            mac_or(
                "Create Stacked Pull Requests",
                "Create stacked pull requests",
            ),
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "stacked-pull-requests-cancel",
                    label: "Cancel".into(),
                    disabled: working,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "stacked-pull-requests-create",
                    label: if working {
                        "Creating…".into()
                    } else {
                        mac_or("Create Pull Requests", "Create pull requests").into()
                    },
                    disabled: working || branches.is_empty(),
                    on_click: Box::new(move |_, cx| {
                        this.update(cx, |this, cx| {
                            this.working = true;
                            cx.notify();
                        })
                        .ok();
                        Dispatcher::create_stacked_pull_requests(repo, draft, cx);
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
