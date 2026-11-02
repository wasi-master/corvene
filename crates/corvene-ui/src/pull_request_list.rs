//! Pull Requests tab of the branch foldout - GHD
//! `ui/branches/{pull-request-list,pull-request-list-item,no-pull-requests}.tsx`
//! (`styles/ui/_branches.scss` `.pull-request-list`, `.no-pull-requests`):
//! 47 px rows with the PR icon, title, "#N opened … by author" subtitle and
//! the CI status, plus the blank slate. The rows live in the
//! `BranchFoldout`; this module renders them.
//!
//! The row context menu is GHD `pull-request-list-item-context-menu.tsx`
//! ("View Pull Request on GitHub", "Checkout in New Worktree…" →
//! `AddWorktree` with the head branch and `<repository>-<number>`).
//!
//! `quick_view` is GHD `ui/pull-request-quick-view.tsx`
//! (`styles/ui/_pull-request-quick-view.scss`, `enablePullRequestQuickView`):
//! a 400 px card right of the foldout with the "Review requested" header and
//! "View on GitHub", the Open / Draft pill, the title with the `#N` + CI
//! badge, and the body as Markdown (max 500 px, scrolling), with a pointer at
//! the hovered row. Corvene adds the list item's "opened … by author" line
//! under the title.
//!
//! Deviation (flag `pull-requests-signed-out`): with no account for the
//! repository's endpoint the empty list shows "Sign in to see pull requests"
//! with a sign-in link and the refresh button is disabled (GHD shows "You're
//! all set!" and a refresh button that does nothing).

use corvene_core::filter::match_keys;
use corvene_core::{AppState, Dispatcher, Popup, PullRequest, parse_iso8601};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ci_status::ci_status;
use crate::context_menu::{MenuItem, mac_or};
use crate::icons::{Octicon, octicon};
use crate::relative_time::relative;
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::ListRowA11y;
use crate::widgets::code_ref;

/// `RowHeight`
#[allow(non_snake_case)]
pub fn PR_ROW_HEIGHT() -> Pixels {
    zpx(47.)
}

/// `opened 3 days ago by octocat`
fn opened_by(pr: &PullRequest) -> String {
    let date = parse_iso8601(&pr.created_at)
        .map(|t| {
            if crate::format::prefer_absolute_dates() {
                crate::format::format_date(t)
            } else {
                relative(t)
            }
        })
        .unwrap_or_default();
    format!("opened {} by {}", date, pr.author)
}

/// `getSubtitle`: `#12 opened 3 days ago by octocat • Draft`.
pub fn subtitle(pr: &PullRequest) -> String {
    // flags 342-344: GitLab numbers merge requests `!12`
    let text = format!("{} {}", pr.number_label(), opened_by(pr));
    if pr.draft {
        format!("{text} • Draft")
    } else {
        text
    }
}

/// `356-stacked-pull-requests`: the quick view's "Stack 2 of 3" with links to
/// the pull requests below and above on GitHub.
fn stack_links(stack: corvene_core::stacked_prs::StackPosition, cx: &App) -> Div {
    let t = cx.ghd();
    let link = |id: &'static str, word: &str, pr: PullRequest| {
        crate::widgets::link_button(id, format!("{word} {}", pr.number_label()), cx)
            .on_click(move |_, _, cx| Dispatcher::open_pull_request(&pr, cx))
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(SPACING())
        .pb(SPACING())
        .text_size(FONT_SIZE_SM())
        .child(
            div()
                .text_color(t.text_secondary)
                .child(format!("Stack {}", stack.label())),
        )
        .when_some(stack.below, |d, pr| {
            d.child(link("quick-view-stack-below", "Below", pr))
        })
        .when_some(stack.above, |d, pr| {
            d.child(link("quick-view-stack-above", "Above", pr))
        })
}

/// The filter list matches the title or the subtitle (GHD `match` over
/// `text: [pr.title, getSubtitle(pr)]`).
pub fn matches_filter(pr: &PullRequest, query: &str) -> bool {
    query.is_empty() || match_keys(query, &[pr.title.clone(), subtitle(pr)]).is_some()
}

/// Corvene (`335-pull-request-list-filters`): which pull requests the list
/// shows besides the filter text (GHD lists them all).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PullRequestListFilter {
    #[default]
    All,
    CreatedByMe,
    ReviewRequested,
    AssignedToMe,
}

impl PullRequestListFilter {
    pub const ALL: [Self; 4] = [
        Self::All,
        Self::CreatedByMe,
        Self::ReviewRequested,
        Self::AssignedToMe,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => mac_or("All Pull Requests", "All pull requests"),
            Self::CreatedByMe => mac_or("Created by Me", "Created by me"),
            Self::ReviewRequested => mac_or("Review Requested from Me", "Review requested from me"),
            Self::AssignedToMe => mac_or("Assigned to Me", "Assigned to me"),
        }
    }

    /// Whether `pr` passes for the signed-in `login` (no login: only All
    /// shows anything).
    pub fn matches(self, pr: &PullRequest, login: Option<&str>) -> bool {
        let me = |user: &str| login.is_some_and(|l| l.eq_ignore_ascii_case(user));
        match self {
            Self::All => true,
            Self::CreatedByMe => me(&pr.author),
            Self::ReviewRequested => pr.requested_reviewers.iter().any(|u| me(u)),
            Self::AssignedToMe => pr.assignees.iter().any(|u| me(u)),
        }
    }
}

/// A pull request's CI status, as `ci_status` draws it.
pub type CiSummary = Option<(
    corvene_core::CheckStatus,
    Option<corvene_core::CheckConclusion>,
)>;

/// `PullRequestListItem`; `repository_name` names the worktree that
/// "Checkout in New Worktree…" proposes.
pub fn pull_request_row(
    id: u64,
    pr: &PullRequest,
    selected: bool,
    status: CiSummary,
    repository_name: String,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    // `.list-item:hover`: `--list-item-hover-background-color`, text unchanged
    let list_hover = t.list_item_hover_background;
    let hover_bg = t.box_selected_active_background;
    let hover_text = t.box_selected_active_text;
    let icon_color = if pr.draft {
        t.pr_draft_icon
    } else {
        t.pr_open_icon
    };
    // `356-stacked-pull-requests`
    let stack_position = corvene_core::AppState::global(cx)
        .read(cx)
        .stack_position(id, pr);
    let pr_for_click = pr.clone();
    let pr_for_menu = pr.clone();
    let pr_for_review = pr.clone();
    let pr_for_drop = pr.clone();
    let pr_for_drop_target = pr.head.ref_name.clone();
    div()
        .id(SharedString::from(format!("pull-request-{}", pr.number)))
        .a11y_row(format!("{}, {}", pr.title, subtitle(pr)), selected)
        .h(PR_ROW_HEIGHT())
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .px(SPACING())
        .cursor_pointer()
        .when(selected, |d| {
            d.bg(t.box_selected_background)
                .text_color(t.box_selected_text)
        })
        // `.list-item:hover` outranks `.list-item.selected` (flag 104 keeps it)
        .when(
            !(selected && crate::widgets::selection_keeps_colour_on_hover(cx)),
            move |d| d.hover(move |s| s.bg(list_hover)),
        )
        // `PullRequestListItem` drop target: dragged commits are copied onto
        // the pull request's branch (`emitEnterDropTarget({ type: Branch })`)
        .when(!selected, |d| {
            let target_name = pr_for_drop_target.clone();
            d.drag_over::<crate::history::CommitDrag>(move |s, _, _, _| {
                s.bg(hover_bg).text_color(hover_text)
            })
            .on_drag_move::<crate::history::CommitDrag>(move |ev, _, cx| {
                if ev.bounds.contains(&ev.event.position) {
                    Dispatcher::set_drag_target(
                        Some(corvene_core::DropTarget::Branch(target_name.clone())),
                        cx,
                    );
                }
            })
        })
        .on_drop({
            // `onDropOntoPullRequest`: the checked-out pull request counts as
            // no target; anything else starts the cherry-pick onto its branch
            let pr = pr_for_drop.clone();
            move |drag: &crate::history::CommitDrag, _, cx| {
                Dispatcher::set_drag_target(None, cx);
                if selected || drag.repo != id {
                    return;
                }
                Dispatcher::close_foldout(cx);
                Dispatcher::start_cherry_pick_flow(id, drag.shas.clone(), cx);
                Dispatcher::cherry_pick_to_pull_request(id, pr.clone(), cx);
            }
        })
        .on_click(move |_, _, cx| {
            Dispatcher::close_foldout(cx);
            Dispatcher::checkout_pull_request(id, pr_for_click.clone(), cx);
        })
        .on_mouse_down(MouseButton::Right, move |ev, window, cx| {
            // `generatePullRequestContextMenuItems`
            let pr = pr_for_menu.clone();
            let head = pr.head.ref_name.clone();
            let worktree_name = format!("{repository_name}-{}", pr.number);
            // Corvene (`348-pull-request-review`): review it in the app
            let review = AppState::global(cx)
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::PULL_REQUEST_REVIEW)
                .then(|| {
                    let pr = pr_for_review.clone();
                    MenuItem::new("Review Pull Request", move |_, cx| {
                        Dispatcher::close_foldout(cx);
                        Dispatcher::review_pull_request(id, pr.clone(), cx);
                    })
                });
            // Corvene (`1223-checkout-from-fork`): another branch of the
            // pull request's fork
            let fork_owner = pr
                .head
                .repository
                .as_ref()
                .filter(|head| {
                    pr.base
                        .repository
                        .as_ref()
                        .is_some_and(|base| !base.owner.eq_ignore_ascii_case(&head.owner))
                })
                .map(|head| head.owner.clone())
                .filter(|_| {
                    AppState::global(cx)
                        .read(cx)
                        .flags
                        .bool(corvene_core::flags::ids::CHECKOUT_FROM_FORK)
                });
            let other_fork_branch = fork_owner.map(|owner| {
                MenuItem::new(
                    if crate::context_menu::IS_MAC {
                        format!("Check Out Another Branch from {owner}'s Fork…")
                    } else {
                        format!("Check out another branch from {owner}'s fork…")
                    },
                    move |_, cx| {
                        Dispatcher::show_checkout_from_fork(id, Some(owner.clone()), None, cx)
                    },
                )
            });
            crate::native_menu::show_context_menu(
                review
                    .into_iter()
                    .chain(vec![
                        MenuItem::new("View Pull Request on GitHub", move |_, cx| {
                            Dispatcher::open_pull_request(&pr, cx)
                        }),
                        MenuItem::new(
                            mac_or("Checkout in New Worktree…", "Checkout in new worktree…"),
                            move |_, cx| {
                                Dispatcher::close_foldout(cx);
                                Dispatcher::show_popup(
                                    Popup::AddWorktree {
                                        repo: id,
                                        initial_branch_name: Some(head.clone()),
                                        initial_worktree_name: Some(worktree_name.clone()),
                                    },
                                    cx,
                                )
                            },
                        ),
                    ])
                    .chain(other_fork_branch)
                    .collect(),
                ev.position,
                window,
                cx,
            );
        })
        .child(
            div()
                .flex_none()
                .self_start()
                .mt(zpx(2.))
                .ml(SPACING_HALF())
                .mr(SPACING())
                .pt(zpx(7.))
                .child(octicon(
                    if pr.draft {
                        Octicon::GitPullRequestDraft
                    } else {
                        Octicon::GitPullRequest
                    },
                    icon_color,
                )),
        )
        .child(
            // `.info`
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .mr(SPACING_HALF())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(FONT_SIZE())
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(pr.title.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(FONT_SIZE_SM())
                        .font_weight(FontWeight::LIGHT)
                        .text_color(if selected {
                            t.box_selected_text
                        } else {
                            t.text_secondary
                        })
                        .child(match stack_position.as_ref() {
                            Some(position) => {
                                format!("{} • {}", subtitle(pr), position.label())
                            }
                            None => subtitle(pr),
                        }),
                ),
        )
        .child(
            // `.ci-status-container`
            div()
                .flex_none()
                .min_w(zpx(16.))
                .mr(SPACING_HALF())
                .flex()
                .justify_center()
                .when_some(status, |d, (status, conclusion)| {
                    d.child(ci_status(status, conclusion))
                }),
        )
}

/// `maxQuickViewHeight`: 500 px body + 56 px header.
#[allow(non_snake_case)]
pub fn QUICK_VIEW_MAX_HEIGHT() -> Pixels {
    zpx(556.)
}
/// `.pull-request-quick-view-contents { min-width: 400px }`
#[allow(non_snake_case)]
fn QUICK_VIEW_WIDTH() -> Pixels {
    zpx(400.)
}

/// `calculatePosition`: the card's top relative to the foldout's top, given
/// the hovered row's top (both window-space) and the window height.
pub fn quick_view_top(
    row_top: Pixels,
    list_top: Pixels,
    window_height: Pixels,
    height: Pixels,
) -> Pixels {
    let clamp = |v: Pixels, min: Pixels, max: Pixels| v.min(max).max(min);
    let (min_top, max_top) = (zpx(0.), window_height - list_top - height);
    if window_height - row_top > height {
        return clamp(row_top - list_top, min_top, max_top);
    }
    if row_top - height > zpx(0.) {
        return clamp(
            row_top - list_top - height + PR_ROW_HEIGHT(),
            min_top,
            max_top,
        );
    }
    let middle = row_top + PR_ROW_HEIGHT() / 2. - height / 2.;
    clamp(middle, min_top, max_top)
}

/// `PullRequestQuickView`: the card (with its 10 px side margins) and the
/// pointer at `pointer_top` (card-relative centre of the hovered row).
pub fn quick_view(
    pr: &PullRequest,
    body: &[corvene_core::markdown::Block],
    status: CiSummary,
    pointer_top: Pixels,
    measured_height: std::rc::Rc<std::cell::Cell<Pixels>>,
    cx: &App,
) -> Stateful<Div> {
    let t = cx.ghd();
    let pill = if pr.draft {
        t.pr_draft_icon
    } else {
        t.pr_open_icon
    };
    let pr_for_view = pr.clone();
    let stack = {
        let s = corvene_core::AppState::global(cx).read(cx);
        s.selected.and_then(|id| s.stack_position(id, pr))
    };
    let (border, background) = (t.box_border, t.background);
    // `301` / `302`: the "opened … by" line and the width rule
    let (show_opened_by, fixed_width) = {
        use corvene_core::flags::ids;
        let state = corvene_core::AppState::global(cx);
        let flags = &state.read(cx).flags;
        (
            flags.bool(ids::PR_QUICK_VIEW_OPENED_BY),
            flags.text(ids::PR_QUICK_VIEW_WIDTH) != "min-400",
        )
    };
    div()
        .id("pull-request-quick-view")
        .relative()
        .occlude()
        .child(crate::widgets::touch_drag_occluder())
        .px(SPACING())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            // `.pull-request-quick-view-contents` (`min-width: 400px`; Corvene
            // fixes the width unless `302-pr-quick-view-width` says otherwise)
            div()
                .relative()
                .when(fixed_width, |d| d.w(QUICK_VIEW_WIDTH()))
                .when(!fixed_width, |d| {
                    d.min_w(QUICK_VIEW_WIDTH()).max_w(zpx(600.))
                })
                .flex()
                .flex_col()
                .bg(t.background)
                .text_color(t.text)
                .text_size(FONT_SIZE())
                .rounded(BORDER_RADIUS())
                .child(
                    canvas(
                        move |b, window, _| {
                            if measured_height.get() != b.size.height {
                                measured_height.set(b.size.height);
                                window.refresh();
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(
                    // `.header`
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .p(SPACING_DOUBLE())
                        .pb(SPACING())
                        .border_b_1()
                        .border_color(t.box_border)
                        .child(octicon(Octicon::ListUnordered, t.text))
                        .child(
                            div()
                                .flex_1()
                                .pl(SPACING_DOUBLE())
                                .child("Review requested"),
                        )
                        .child(
                            crate::widgets::button("quick-view-on-github", "", cx)
                                .flex_none()
                                .gap(SPACING_HALF())
                                .role(Role::Link)
                                .aria_label(format!("View on {}", pr.host_site_name()))
                                .child(format!("View on {}", pr.host_site_name()))
                                .child(octicon(Octicon::LinkExternal, t.secondary_button_text))
                                .on_click(move |_, _, cx| {
                                    Dispatcher::open_pull_request(&pr_for_view, cx)
                                }),
                        ),
                )
                .child(
                    // `.pull-request`
                    div()
                        .id("quick-view-pull-request")
                        .max_h(zpx(500.))
                        .overflow_y_scroll()
                        .p(SPACING_DOUBLE())
                        .child(
                            // `.status`
                            div().flex().flex_row().child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .py(SPACING_HALF())
                                    .px(SPACING())
                                    .rounded(zpx(28.))
                                    .bg(pill)
                                    .text_color(gpui_kit::white())
                                    .text_size(FONT_SIZE_MD())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(octicon(
                                        if pr.draft {
                                            Octicon::GitPullRequestDraft
                                        } else {
                                            Octicon::GitPullRequest
                                        },
                                        gpui_kit::white(),
                                    ))
                                    .child(div().ml(SPACING_HALF()).child(if pr.draft {
                                        "Draft"
                                    } else {
                                        "Open"
                                    })),
                            ),
                        )
                        .child(
                            // `.title`: h2 + `PullRequestBadge`
                            div()
                                .my(SPACING())
                                .flex()
                                .flex_col()
                                .gap(SPACING_HALF())
                                .child(
                                    div()
                                        .text_size(zpx(18.))
                                        .line_height(zpx(24.))
                                        .font_weight(FontWeight::BOLD)
                                        .child(pr.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(SPACING_HALF())
                                        .child(
                                            div()
                                                .flex_none()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .h(zpx(22.))
                                                .px(SPACING_HALF())
                                                .rounded(BORDER_RADIUS())
                                                .border_1()
                                                .border_color(t.box_border)
                                                .text_size(FONT_SIZE_SM())
                                                .child(pr.number_label())
                                                .when_some(status, |d, (status, conclusion)| {
                                                    d.child(
                                                        ci_status(status, conclusion)
                                                            .ml(SPACING_HALF()),
                                                    )
                                                }),
                                        )
                                        .when(show_opened_by, |d| {
                                            d.child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(FONT_SIZE_SM())
                                                    .text_color(t.text_secondary)
                                                    .child(opened_by(pr)),
                                            )
                                        }),
                                ),
                        )
                        // `356-stacked-pull-requests`
                        .when_some(stack, |d, stack| d.child(stack_links(stack, cx)))
                        .child(crate::markdown::markdown(
                            "quick-view-body",
                            body,
                            pr.base.repository.as_ref().map(|r| r.html_url.as_str()),
                            cx,
                        ))
                        .with_scrollbar(),
                ),
        )
        .child(
            // `.pull-request-pointer`: a left-pointing triangle outlined in
            // the border colour, 7 px above the row's centre
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    let o = b.origin;
                    let tri = |dx: f32, half: f32| {
                        let mut path = Path::new(point(o.x + zpx(dx), o.y + zpx(8.)));
                        path.line_to(point(o.x + zpx(dx + half), o.y + zpx(8. - half)));
                        path.line_to(point(o.x + zpx(dx + half), o.y + zpx(8. + half)));
                        path.line_to(point(o.x + zpx(dx), o.y + zpx(8.)));
                        path
                    };
                    window.paint_path(tri(2., 8.), border);
                    window.paint_path(tri(3.5, 7.), background);
                },
            )
            .absolute()
            .left(zpx(0.))
            .top(pointer_top - zpx(8.))
            .w(zpx(12.))
            .h(zpx(16.)),
        )
}

/// The blank slate when no account can load the pull requests (flag
/// `pull-requests-signed-out`; GHD shows `NoPullRequests`' "You're all
/// set!"): a sign-in prompt for the repository's endpoint.
pub fn signed_out_pull_requests(repository_name: String, endpoint: String, cx: &App) -> AnyElement {
    let t = cx.ghd();
    let enterprise = endpoint != "https://api.github.com";
    let service = if enterprise {
        "GitHub Enterprise"
    } else {
        "GitHub.com"
    };
    div()
        .id("no-pull-requests-signed-out")
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .p(SPACING())
        .text_size(FONT_SIZE())
        .child(
            crate::widgets::blankslate_image("empty-no-pull-requests.svg", cx)
                .w(zpx(200.))
                .mb(SPACING()),
        )
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Sign in to see pull requests"),
        )
        .child(
            div()
                .pb(SPACING())
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .justify_center()
                .gap(zpx(3.))
                .child("Pull requests in")
                .child(code_ref(repository_name, cx))
                .child(format!("are loaded with a {service} account.")),
        )
        .child(
            div().text_size(FONT_SIZE_SM()).text_color(t.text).child(
                crate::widgets::link_button("no-prs-sign-in", format!("Sign in to {service}"), cx)
                    .on_click(move |_, _, cx| {
                        Dispatcher::close_foldout(cx);
                        Dispatcher::show_popup(Popup::SignIn { enterprise }, cx)
                    }),
            ),
        )
        .into_any_element()
}

/// What a `NoPullRequests` link button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPullRequestsAction {
    /// `onCreateBranch`
    CreateBranch,
    /// `onCreatePullRequest`
    CreatePullRequest,
}

impl NoPullRequestsAction {
    /// The text around the link button: "Would you like to <link> …".
    pub fn sentence(self) -> (&'static str, &'static str) {
        match self {
            Self::CreateBranch => ("Would you like to ", " and get going on your next project?"),
            Self::CreatePullRequest => ("Would you like to ", " from the current branch?"),
        }
    }
}

/// GHD `NoPullRequests.renderCallToAction`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoPullRequestsCallToAction {
    /// The `.call-to-action` text alone.
    Text(String),
    /// "Would you like to <link button> …": the link button's label and what
    /// it does (the surrounding text is [`NoPullRequestsAction::sentence`]).
    Link(String, NoPullRequestsAction),
}

/// What GHD `NoPullRequests` (`ui/branches/no-pull-requests.tsx`) shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoPullRequestsContent {
    /// Whether the `.blankslate-image` (`alt=""`) is drawn.
    pub blankslate_image: bool,
    /// `.title`
    pub title: String,
    /// `.no-prs`: its text and the repository name (`<Ref>`), when neither
    /// searching nor loading.
    pub no_prs: Option<(String, String)>,
    pub call_to_action: NoPullRequestsCallToAction,
}

/// GHD `INoPullRequestsProps` (the callbacks are [`NoPullRequestsAction`]s).
pub struct NoPullRequestsProps<'a> {
    pub repository_name: &'a str,
    pub is_on_default_branch: bool,
    pub is_search: bool,
    pub is_loading_pull_requests: bool,
}

/// GHD `NoPullRequests` (`renderTitle`, `renderCallToAction`).
pub fn no_pull_requests_content(props: NoPullRequestsProps<'_>) -> NoPullRequestsContent {
    let (title, no_prs) = if props.is_search {
        ("Sorry, I can't find that pull request!", None)
    } else if props.is_loading_pull_requests {
        ("Hang tight", None)
    } else {
        (
            "You're all set!",
            Some((
                "No open pull requests in".to_string(),
                props.repository_name.to_string(),
            )),
        )
    };
    let call_to_action = if props.is_loading_pull_requests {
        NoPullRequestsCallToAction::Text("Loading pull requests as fast as I can!".into())
    } else if props.is_on_default_branch {
        NoPullRequestsCallToAction::Link(
            "create a new branch".into(),
            NoPullRequestsAction::CreateBranch,
        )
    } else {
        NoPullRequestsCallToAction::Link(
            "create a pull request".into(),
            NoPullRequestsAction::CreatePullRequest,
        )
    };
    NoPullRequestsContent {
        blankslate_image: true,
        title: title.into(),
        no_prs,
        call_to_action,
    }
}

/// `NoPullRequests`, drawn from [`no_pull_requests_content`].
pub fn no_pull_requests(
    id: u64,
    repository_name: String,
    is_search: bool,
    loading: bool,
    on_default_branch: bool,
    cx: &App,
) -> AnyElement {
    let t = cx.ghd();
    let NoPullRequestsContent {
        blankslate_image,
        title,
        no_prs,
        call_to_action,
    } = no_pull_requests_content(NoPullRequestsProps {
        repository_name: &repository_name,
        is_on_default_branch: on_default_branch,
        is_search,
        is_loading_pull_requests: loading,
    });
    // flag `342-gitlab`: GitLab's word for them
    let merge_requests = corvene_core::AppState::try_global(cx)
        .and_then(|s| s.read(cx).hosted_repository(id))
        .is_some_and(|h| h.kind.is_merge_request());
    let words = |text: String| {
        if merge_requests {
            text.replace("pull request", "merge request")
        } else {
            text
        }
    };
    let title = words(title);
    let no_prs = no_prs.map(|(text, name)| (words(text), name));
    let call_to_action = match call_to_action {
        NoPullRequestsCallToAction::Text(text) => NoPullRequestsCallToAction::Text(words(text)),
        NoPullRequestsCallToAction::Link(label, action) => {
            NoPullRequestsCallToAction::Link(words(label), action)
        }
    };
    let call_to_action: AnyElement = match call_to_action {
        NoPullRequestsCallToAction::Text(text) => div().child(text).into_any_element(),
        NoPullRequestsCallToAction::Link(label, action) => {
            let (lead, tail) = action.sentence();
            let link_id = match action {
                NoPullRequestsAction::CreateBranch => "no-prs-create-branch",
                NoPullRequestsAction::CreatePullRequest => "no-prs-create-pr",
            };
            crate::widgets::paragraph(vec![
                lead.into(),
                crate::widgets::Inline::Element(
                    crate::widgets::link_button(link_id, label, cx)
                        .on_click(move |_, _, cx| {
                            Dispatcher::close_foldout(cx);
                            match action {
                                NoPullRequestsAction::CreateBranch => Dispatcher::show_popup(
                                    Popup::CreateBranch {
                                        repo: id,
                                        target_sha: None,
                                        initial_name: String::new(),
                                    },
                                    cx,
                                ),
                                NoPullRequestsAction::CreatePullRequest => {
                                    Dispatcher::create_pull_request(id, cx)
                                }
                            }
                        })
                        .into_any_element(),
                ),
                tail.into(),
            ])
            .justify_center()
            .into_any_element()
        }
    };
    div()
        .id("no-pull-requests")
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .p(SPACING())
        .text_size(FONT_SIZE())
        .when(blankslate_image, |d| {
            d.child(
                crate::widgets::blankslate_image("empty-no-pull-requests.svg", cx)
                    .w(zpx(200.))
                    .mb(SPACING()),
            )
        })
        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
        .when_some(no_prs, |d, (text, repository_name)| {
            d.child(
                div()
                    .pb(SPACING())
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(zpx(3.))
                    .child(text)
                    .child(code_ref(repository_name, cx)),
            )
        })
        .child(
            div()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text)
                .child(call_to_action),
        )
        .into_any_element()
}
