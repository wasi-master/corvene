//! Corvene `348-pull-request-review`: the pull request's overview in the
//! commit view's place (`corvene_core::pull_request_review`). GitHub
//! Desktop shows a pull request only as a list row and in the browser.
//!
//! The title with the state pill (Open, Draft, Merged, Closed), who opened
//! it when and from which branch into which, the size (+N −M, files,
//! commits); the labels, milestone, assignees, requested reviewers and the
//! reviews given (an icon per verdict), mergeability and the review
//! decision; the checks of the head (the commit status store's, as the CI
//! popover lists them); the description as Markdown; the timeline (commits,
//! comments, reviews, requests, labels, renames, force-pushes, merges…),
//! newest last; and a box for a comment on the conversation.

use std::sync::Arc;

use corvene_core::pull_request_review::{
    OverviewReview, PullRequestOverview, PullRequestReviewState, ReviewRequest, RollupState,
    TimelineItem,
};
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::github_list::{self, date_text, label_chip, pill, state_pill};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, c, primer};
use crate::widgets::{GhdTooltip, avatar_image, avatar_lookup_url, primary_button};

fn avatar_size() -> Pixels {
    zpx(20.)
}

pub struct PullRequestOverviewView {
    state: Entity<AppState>,
    scroll: ScrollHandle,
    /// The pull request whose overview was last laid out (resets the scroll).
    shown: Option<u64>,
    comment: Option<Entity<TextareaState>>,
    /// Avatar URLs asked for.
    avatars: std::collections::HashSet<String>,
}

impl PullRequestOverviewView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            scroll: ScrollHandle::new(),
            shown: None,
            comment: None,
            avatars: Default::default(),
        }
    }

    fn comment_box(
        &mut self,
        repo: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        if let Some(input) = &self.comment {
            return input.clone();
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Leave a comment")
                .rows(3)
        });
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&input, move |this, input, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
            {
                this.post_comment(repo, &input, cx);
            }
        })
        .detach();
        self.comment = Some(input.clone());
        input
    }

    fn post_comment(&mut self, repo: u64, input: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let text = input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        Dispatcher::add_pull_request_conversation_comment(repo, text, cx);
        // the box is remade empty on the next render
        self.comment = None;
        cx.notify();
    }

    fn request_avatars(&mut self, overview: &PullRequestOverview, cx: &mut Context<Self>) {
        let mut wanted: Vec<String> = Vec::new();
        let mut want = |url: Option<&String>| {
            if let Some(url) = url
                && !url.is_empty()
                && !self.avatars.contains(url)
            {
                self.avatars.insert(url.clone());
                wanted.push(url.clone());
            }
        };
        want(overview.author.as_ref().and_then(|a| a.avatar_url.as_ref()));
        for r in &overview.reviews {
            want(r.author.as_ref().and_then(|a| a.avatar_url.as_ref()));
        }
        for r in &overview.review_requests {
            want(r.avatar_url.as_ref());
        }
        for item in &overview.timeline {
            match item {
                TimelineItem::Comment { author, .. } => {
                    want(author.as_ref().and_then(|a| a.avatar_url.as_ref()))
                }
                TimelineItem::Review(r) => {
                    want(r.author.as_ref().and_then(|a| a.avatar_url.as_ref()))
                }
                _ => {}
            }
        }
        if !wanted.is_empty() {
            cx.spawn(async move |_, cx| {
                cx.update(|cx| {
                    for url in wanted {
                        Dispatcher::request_avatar_url(&url, cx);
                    }
                });
            })
            .detach();
        }
    }
}

/// The verdict icon of a review (`pull-request-review-helpers.ts`).
fn review_icon(state: &str, t: &GhdTheme) -> (Octicon, Hsla, Hsla) {
    match state {
        "APPROVED" => (
            Octicon::Check,
            t.pr_approved_icon,
            t.pr_approved_icon_background,
        ),
        "CHANGES_REQUESTED" => (
            Octicon::FileDiff,
            t.pr_changes_requested_icon,
            t.pr_changes_requested_icon_background,
        ),
        _ => (
            Octicon::Eye,
            t.pr_commented_icon,
            t.pr_commented_icon_background,
        ),
    }
}

fn review_verb(state: &str) -> &'static str {
    match state {
        "APPROVED" => "approved",
        "CHANGES_REQUESTED" => "requested changes",
        "DISMISSED" => "left a review that was dismissed",
        _ => "reviewed",
    }
}

/// A 16 px round verdict badge.
fn verdict_badge(state: &str, t: &GhdTheme) -> Div {
    let (icon, fg, bg) = review_icon(state, t);
    div()
        .flex_none()
        .size(zpx(18.))
        .rounded_full()
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(octicon(icon, fg).size(zpx(11.)))
}

fn avatar_of(url: Option<&str>, cx: &App) -> AnyElement {
    avatar_image(
        url.and_then(|u| avatar_lookup_url(u, cx)),
        avatar_size(),
        cx,
    )
}

/// A small titled block of the meta section.
fn meta_block(title: &'static str, content: impl IntoElement, t: &GhdTheme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(zpx(3.))
        .min_w(zpx(140.))
        .child(
            div()
                .text_size(FONT_SIZE_SM())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_secondary)
                .child(title),
        )
        .child(content)
}

fn person(login: &str, avatar: Option<&str>, cx: &App) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(zpx(4.))
        .child(avatar_of(avatar, cx))
        .child(login.to_string())
}

impl PullRequestOverviewView {
    fn header(
        &self,
        review: &PullRequestReviewState,
        overview: Option<&PullRequestOverview>,
        t: &GhdTheme,
        cx: &App,
    ) -> Div {
        let pr = &review.pull_request;
        let (state_text, icon, color) = match overview.map(|o| (o.state.as_str(), o.is_draft)) {
            Some(("MERGED", _)) => ("Merged", Octicon::GitMerge, c(primer::PURPLE_500)),
            Some(("CLOSED", _)) => (
                "Closed",
                Octicon::GitPullRequest,
                github_list::closed_color(t),
            ),
            Some((_, true)) | None if pr.draft => {
                ("Draft", Octicon::GitPullRequestDraft, t.pr_draft_icon)
            }
            _ => ("Open", Octicon::GitPullRequest, t.pr_open_icon),
        };
        let author = overview
            .and_then(|o| o.author.as_ref().map(|a| a.login.clone()))
            .unwrap_or_else(|| pr.author.clone());
        let opened = date_text(Some(
            overview
                .map(|o| o.created_at.as_str())
                .unwrap_or(pr.created_at.as_str()),
        ));
        let (base, head) = match overview {
            Some(o) => (o.base_ref_name.clone(), o.head_ref_name.clone()),
            None => (pr.base.ref_name.clone(), pr.head.ref_name.clone()),
        };
        let head = match overview
            .and_then(|o| o.head_repository.clone())
            .filter(|_| overview.is_some_and(|o| o.is_cross_repository))
        {
            Some(repo) => format!("{repo}:{head}"),
            None => head,
        };
        let mut meta = format!("{author} wants to merge");
        if let Some(o) = overview {
            meta.push_str(&format!(
                " {} commit{}",
                o.commit_count,
                if o.commit_count == 1 { "" } else { "s" }
            ));
        }
        meta.push_str(&format!(" into {base} from {head}"));
        if !opened.is_empty() {
            meta.push_str(&format!(" • opened {opened}"));
        }
        let mut size: Vec<AnyElement> = Vec::new();
        if let Some(o) = overview {
            size.push(
                div()
                    .text_color(t.color_new)
                    .child(format!("+{}", o.additions))
                    .into_any_element(),
            );
            size.push(
                div()
                    .text_color(t.color_deleted)
                    .child(format!("−{}", o.deletions))
                    .into_any_element(),
            );
            size.push(
                div()
                    .text_color(t.text_secondary)
                    .child(format!(
                        "{} file{}",
                        o.changed_files,
                        if o.changed_files == 1 { "" } else { "s" }
                    ))
                    .into_any_element(),
            );
        }
        if let Some(branch) = &review.local_branch {
            size.push(pill(format!("checked out: {branch}"), t.text_secondary).into_any_element());
        } else if review.display_head.is_some() {
            size.push(pill("GitHub's head", t.text_secondary).into_any_element());
        }
        if review.display_head.is_some() && !review.same_head() {
            size.push(
                pill("local commits ahead of GitHub", c(primer::YELLOW_700)).into_any_element(),
            );
        }
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .p(SPACING())
            .border_b_1()
            .border_color(t.box_border)
            .bg(t.box_alt_background)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(SPACING_HALF())
                    .text_size(FONT_SIZE_LG())
                    .line_height(zpx(22.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(1.)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(pr.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(t.text_secondary)
                            .child(format!("#{}", pr.number)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(state_pill(state_text, icon, color, cx))
                    .child(div().min_w_0().text_color(t.text_secondary).child(meta)),
            )
            .when(!size.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(SPACING_HALF())
                        .text_size(FONT_SIZE_SM())
                        .children(size),
                )
            })
    }

    fn meta(&self, overview: &PullRequestOverview, t: &GhdTheme, cx: &App) -> Div {
        let mut blocks: Vec<AnyElement> = Vec::new();
        // reviewers: requested, and the verdicts given
        let mut reviewers: Vec<AnyElement> = Vec::new();
        for (i, r) in overview.reviews.iter().enumerate() {
            let login = r
                .author
                .as_ref()
                .map(|a| a.login.clone())
                .unwrap_or_else(|| "ghost".into());
            reviewers.push(
                div()
                    .id(("overview-reviewer", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(4.))
                    .child(avatar_of(
                        r.author.as_ref().and_then(|a| a.avatar_url.as_deref()),
                        cx,
                    ))
                    .child(login.clone())
                    .child(verdict_badge(&r.state, t))
                    .ghd_tooltip(format!("{login} {}", review_verb(&r.state)))
                    .into_any_element(),
            );
        }
        for (i, r) in overview.review_requests.iter().enumerate() {
            let ReviewRequest {
                name,
                avatar_url,
                team,
            } = r;
            reviewers.push(
                div()
                    .id(("overview-requested", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(4.))
                    .child(if *team {
                        octicon(Octicon::Person, t.text_secondary).into_any_element()
                    } else {
                        avatar_of(avatar_url.as_deref(), cx)
                    })
                    .child(name.clone())
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child("awaiting"),
                    )
                    .ghd_tooltip(format!("Review requested from {name}"))
                    .into_any_element(),
            );
        }
        if !reviewers.is_empty() {
            blocks.push(
                meta_block(
                    "Reviewers",
                    div().flex().flex_col().gap(zpx(3.)).children(reviewers),
                    t,
                )
                .into_any_element(),
            );
        }
        if !overview.assignees.is_empty() {
            blocks.push(
                meta_block(
                    "Assignees",
                    div()
                        .flex()
                        .flex_col()
                        .gap(zpx(3.))
                        .children(overview.assignees.iter().map(|a| person(a, None, cx))),
                    t,
                )
                .into_any_element(),
            );
        }
        if !overview.labels.is_empty() {
            blocks.push(
                meta_block(
                    "Labels",
                    div().flex().flex_row().flex_wrap().gap(zpx(4.)).children(
                        overview.labels.iter().enumerate().map(|(i, l)| {
                            label_chip(("overview-label", i), &l.name, &l.color, false, cx)
                        }),
                    ),
                    t,
                )
                .into_any_element(),
            );
        }
        if let Some(m) = &overview.milestone {
            blocks.push(meta_block("Milestone", div().child(m.clone()), t).into_any_element());
        }
        // merge state and the review decision
        let mut status: Vec<AnyElement> = Vec::new();
        let decision = match overview.review_decision.as_deref() {
            Some("APPROVED") => Some(("Approved", t.pr_approved_icon_background)),
            Some("CHANGES_REQUESTED") => {
                Some(("Changes requested", t.pr_changes_requested_icon_background))
            }
            Some("REVIEW_REQUIRED") => Some(("Review required", c(primer::YELLOW_700))),
            _ => None,
        };
        if let Some((text, color)) = decision {
            status.push(pill(text, color).into_any_element());
        }
        let merge = match overview.mergeable.as_str() {
            "MERGEABLE" => Some(("No conflicts with the base branch", t.pr_open_icon)),
            "CONFLICTING" => Some(("Conflicts with the base branch", t.dialog_error)),
            _ => None,
        };
        if let Some((text, color)) = merge.filter(|_| overview.state == "OPEN") {
            status.push(pill(text, color).into_any_element());
        }
        if let Some(rollup) = overview.checks {
            let (text, color) = match rollup {
                RollupState::Success => ("All checks have passed", t.pr_open_icon),
                RollupState::Pending | RollupState::Expected => {
                    ("Some checks haven't completed yet", c(primer::YELLOW_700))
                }
                RollupState::Failure | RollupState::Error => {
                    ("Some checks were not successful", t.dialog_error)
                }
            };
            status.push(pill(text, color).into_any_element());
        }
        if !status.is_empty() {
            blocks.push(
                meta_block(
                    "Status",
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(zpx(4.))
                        .children(status),
                    t,
                )
                .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(SPACING_DOUBLE())
            .children(blocks)
    }

    /// The head's checks as the commit status store has them.
    fn checks(
        &self,
        review: &PullRequestReviewState,
        t: &GhdTheme,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let gh = review.pull_request.base.repository.clone()?;
        let git_ref = review.pull_request.commit_ref();
        Dispatcher::touch_commit_status(&gh, &git_ref, review.local_branch.clone(), cx);
        let s = self.state.read(cx);
        let check = s.commit_status(&gh, &git_ref)?;
        if check.checks.is_empty() {
            return None;
        }
        let conclusions: Vec<_> = check.checks.iter().map(|c| c.conclusion).collect();
        let summary = corvene_core::commit_status::combined_status_summary(&conclusions, "check");
        let mut list = div()
            .flex()
            .flex_col()
            .rounded(BORDER_RADIUS())
            .border_1()
            .border_color(t.box_border)
            .overflow_hidden();
        for (i, c) in check.checks.iter().enumerate() {
            let url = c.html_url.clone();
            list = list.child(
                div()
                    .id(("overview-check", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .px(SPACING())
                    .h(zpx(28.))
                    .when(i + 1 < check.checks.len(), |d| {
                        d.border_b_1().border_color(t.box_border)
                    })
                    .when_some(url, |d, url| {
                        d.cursor_pointer()
                            .hover(move |s| s.bg(t.box_selected_background))
                            .on_click(move |_, _, cx| Dispatcher::open_url(&url, cx))
                    })
                    .child(crate::ci_status::ci_status(c.status, c.conclusion).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(c.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.5))
                            .truncate()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(c.description.clone()),
                    ),
            );
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Checks: {summary}")),
                )
                .child(list),
        )
    }

    fn timeline(
        &self,
        review: &PullRequestReviewState,
        overview: &PullRequestOverview,
        t: &GhdTheme,
        cx: &App,
    ) -> Div {
        let line = t.pr_timeline_line;
        let mut items: Vec<AnyElement> = Vec::new();
        if overview.timeline_more > 0 {
            items.push(
                div()
                    .text_color(t.text_secondary)
                    .text_size(FONT_SIZE_SM())
                    .child(format!(
                        "{} earlier event{} on GitHub",
                        overview.timeline_more,
                        if overview.timeline_more == 1 { "" } else { "s" }
                    ))
                    .into_any_element(),
            );
        }
        let event = |icon: Octicon, text: String, date: &str, t: &GhdTheme| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .child(
                    div()
                        .flex_none()
                        .size(zpx(22.))
                        .rounded_full()
                        .bg(t.box_alt_background)
                        .border_1()
                        .border_color(line)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(octicon(icon, t.text_secondary).size(zpx(12.))),
                )
                .child(div().flex_1().min_w_0().truncate().child(text))
                .child(
                    div()
                        .flex_none()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(date_text(Some(date))),
                )
                .into_any_element()
        };
        let bubble = |id: &str,
                      author: Option<(&str, Option<&str>)>,
                      verb: String,
                      badge: Option<Div>,
                      body: Option<Arc<Vec<corvene_core::markdown::Block>>>,
                      url: &str,
                      date: &str| {
            let (login, avatar) = author.unwrap_or(("ghost", None));
            let mut head = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(4.))
                .px(SPACING())
                .py(SPACING_HALF())
                .bg(t.box_alt_background)
                .border_b_1()
                .border_color(t.box_border)
                .child(avatar_of(avatar, cx))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(login.to_string()),
                )
                .child(div().text_color(t.text_secondary).child(verb))
                .children(badge)
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(date_text(Some(date))),
                );
            if body.is_none() {
                head = head.border_b_0();
            }
            div()
                .flex()
                .flex_col()
                .rounded(BORDER_RADIUS())
                .border_1()
                .border_color(t.box_border)
                .overflow_hidden()
                .child(head)
                .when_some(body, |d, body| {
                    d.child(div().p(SPACING()).child(crate::markdown::markdown(
                        format!("timeline-{id}"),
                        &body,
                        Some(url),
                        cx,
                    )))
                })
                .into_any_element()
        };
        for item in &overview.timeline {
            items.push(match item {
                TimelineItem::Commit {
                    oid,
                    headline,
                    author,
                    date,
                } => event(
                    Octicon::GitCommit,
                    format!(
                        "{author} committed {headline} ({})",
                        &oid[..oid.len().min(7)]
                    ),
                    date,
                    t,
                ),
                TimelineItem::Comment {
                    id,
                    author,
                    body,
                    created_at,
                    url,
                } => bubble(
                    id,
                    author
                        .as_ref()
                        .map(|a| (a.login.as_str(), a.avatar_url.as_deref())),
                    "commented".into(),
                    None,
                    Some(review.body(id, body)),
                    url,
                    created_at,
                ),
                TimelineItem::Review(OverviewReview {
                    id,
                    author,
                    state,
                    body,
                    submitted_at,
                    url,
                    comment_count,
                }) => bubble(
                    id,
                    author
                        .as_ref()
                        .map(|a| (a.login.as_str(), a.avatar_url.as_deref())),
                    if *comment_count > 0 {
                        format!(
                            "{} with {comment_count} comment{}",
                            review_verb(state),
                            if *comment_count == 1 { "" } else { "s" }
                        )
                    } else {
                        review_verb(state).to_string()
                    },
                    Some(verdict_badge(state, t)),
                    (!body.trim().is_empty()).then(|| review.body(id, body)),
                    url,
                    submitted_at.as_deref().unwrap_or_default(),
                ),
                TimelineItem::ReviewRequested {
                    actor,
                    reviewer,
                    date,
                } => event(
                    Octicon::Eye,
                    format!("{actor} requested a review from {reviewer}"),
                    date,
                    t,
                ),
                TimelineItem::ReviewDismissed { actor, date } => {
                    event(Octicon::X, format!("{actor} dismissed a review"), date, t)
                }
                TimelineItem::Labeled {
                    actor,
                    label,
                    added,
                    date,
                } => event(
                    Octicon::Tag,
                    format!(
                        "{actor} {} the {} label",
                        if *added { "added" } else { "removed" },
                        label.name
                    ),
                    date,
                    t,
                ),
                TimelineItem::Renamed {
                    actor,
                    from,
                    to,
                    date,
                } => event(
                    Octicon::Pencil,
                    format!("{actor} changed the title from \"{from}\" to \"{to}\""),
                    date,
                    t,
                ),
                TimelineItem::ForcePushed {
                    actor,
                    before,
                    after,
                    date,
                } => event(
                    Octicon::RepoPush,
                    format!("{actor} force-pushed from {before} to {after}"),
                    date,
                    t,
                ),
                TimelineItem::ReadyForReview { actor, date } => event(
                    Octicon::GitPullRequest,
                    format!("{actor} marked this pull request as ready for review"),
                    date,
                    t,
                ),
                TimelineItem::ConvertedToDraft { actor, date } => event(
                    Octicon::GitPullRequestDraft,
                    format!("{actor} converted this to a draft"),
                    date,
                    t,
                ),
                TimelineItem::Merged { actor, into, date } => event(
                    Octicon::GitMerge,
                    format!("{actor} merged this pull request into {into}"),
                    date,
                    t,
                ),
                TimelineItem::Closed { actor, date } => {
                    event(Octicon::X, format!("{actor} closed this"), date, t)
                }
                TimelineItem::Reopened { actor, date } => event(
                    Octicon::IssueReopened,
                    format!("{actor} reopened this"),
                    date,
                    t,
                ),
                TimelineItem::HeadRefDeleted { actor, date } => event(
                    Octicon::GitBranch,
                    format!("{actor} deleted the head branch"),
                    date,
                    t,
                ),
            });
        }
        div()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Timeline"))
            .children(items)
    }
}

impl Render for PullRequestOverviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let Some((id, number)) = ({
            let s = self.state.read(cx);
            s.selected.and_then(|id| {
                let review = crate::review_threads::review_state_of(s, id)?;
                Some((id, review.number()))
            })
        }) else {
            return div().size_full().bg(t.background).into_any_element();
        };
        if self.shown != Some(number) {
            self.shown = Some(number);
            self.comment = None;
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        let comment_box = self.comment_box(id, window, cx);
        // the pieces, read then drawn (the checks touch the status store)
        let (
            header,
            meta,
            body,
            timeline,
            loading,
            error,
            signed_out,
            busy,
            can_post,
            overview_for_avatars,
        ) = {
            let s = self.state.read(cx);
            let Some(review) = crate::review_threads::review_state_of(s, id) else {
                return div().size_full().bg(t.background).into_any_element();
            };
            let overview = review.overview.as_ref();
            let header = self.header(review, overview, &t, cx);
            let meta = overview.map(|o| self.meta(o, &t, cx));
            let body = overview.map(|o| {
                let blocks = review.body(&o.id, &o.body);
                if blocks.is_empty() {
                    div()
                        .text_color(t.text_secondary)
                        .child("No description provided.")
                        .into_any_element()
                } else {
                    // `1333-markdown-math`: parsed again with math
                    match crate::markdown::github_options(cx) {
                        Some(_) => crate::markdown::github_markdown(
                            "pr-overview-body",
                            &o.body,
                            Some(&o.url),
                            cx,
                        ),
                        None => {
                            crate::markdown::markdown("pr-overview-body", &blocks, Some(&o.url), cx)
                        }
                    }
                    .into_any_element()
                }
            });
            let timeline = overview.map(|o| self.timeline(review, o, &t, cx));
            (
                header,
                meta,
                body,
                timeline,
                review.overview_loading && overview.is_none(),
                review.overview_error.clone().filter(|_| overview.is_none()),
                review.signed_out,
                review.busy.is_some(),
                review.pull_request_id.is_some() && !review.signed_out && review.busy.is_none(),
                overview.cloned(),
            )
        };
        let checks = {
            let s = self.state.read(cx);
            let review = crate::review_threads::review_state_of(s, id).cloned();
            review.and_then(|r| self.checks(&r, &t, cx))
        };
        if let Some(o) = &overview_for_avatars {
            self.request_avatars(o, cx);
        }
        let input = comment_box.clone();
        let comment = div()
            .flex()
            .flex_col()
            .gap(SPACING_HALF())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Add a comment"),
            )
            .child(Textarea::new(&comment_box))
            .child(div().flex().flex_row().justify_end().child(
                primary_button("overview-comment", "Comment", !can_post, cx).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.post_comment(id, &input, cx);
                    },
                )),
            ));
        let content = div()
            .id("pr-overview-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(SPACING())
            .flex()
            .flex_col()
            .gap(SPACING_DOUBLE())
            .text_size(FONT_SIZE())
            .line_height(zpx(18.))
            .when(loading, |d| {
                d.child(github_list::loading("the pull request", "overview-spin", &t))
            })
            .when_some(error, |d, message| {
                d.child(github_list::error(
                    message,
                    move |_, cx| Dispatcher::load_review_overview(id, cx),
                    cx,
                ))
            })
            .when(signed_out, |d| {
                d.child(
                    div()
                        .text_color(t.text_secondary)
                        .child("Sign in to the repository's GitHub account to read the description, reviews and timeline."),
                )
            })
            .children(meta)
            .children(checks)
            .children(body)
            .children(timeline)
            .when(!signed_out && !loading, |d| d.child(comment))
            .when(busy, |d| d.opacity(0.8));
        div()
            .id("pr-overview")
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .child(header)
            .child(content.with_scrollbar_handle(&self.scroll))
            .into_any_element()
    }
}
