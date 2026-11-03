//! Corvene `1223-checkout-from-fork` (desktop/desktop#17939): Branch ›
//! Check Out from Fork… (`corvene_core::fork_checkout`). GitHub Desktop
//! has no such dialog; fork branches are only reachable through their
//! pull requests.
//!
//! The owner field takes `alice`, `alice/name`, `alice:branch` or a URL;
//! while it differs from the last lookup it lists matching suggestions
//! (owners of pull requests from forks, then the newest forks). Find (or
//! Return) looks the fork up and lists its branches, filtered as you type;
//! Check Out Branch checks the picked one out as `<owner>/<branch>`.

use corvene_core::fork_checkout::{ForkCheckoutState, ForkLookup};
use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::github_list;
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme};
use crate::widgets::{button, filter_text_box, labeled, text_box};

/// Branch rows listed at most; a filter finds the rest.
const MAX_ROWS: usize = 300;

pub struct CheckoutFromForkDialog {
    state: Entity<AppState>,
    repo: u64,
    owner: Entity<InputState>,
    filter: Entity<InputState>,
    /// The branch picked in the list.
    selected: Option<String>,
}

impl CheckoutFromForkDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        input: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&state, |this, _, cx| {
            // the branch the input named, once its fork's branches load
            if this.selected.is_none() {
                this.selected = this
                    .with_fork(cx, |f| match &f.lookup {
                        ForkLookup::Found(source) => source.preselect.clone(),
                        _ => None,
                    })
                    .flatten();
            }
            cx.notify();
        })
        .detach();
        let owner = cx.new(|cx| {
            let mut field =
                InputState::new(window, cx).placeholder("Owner, owner/name, owner:branch or URL");
            field.set_value(input, window, cx);
            field
        });
        cx.observe(&owner, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&owner, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.find(cx);
            }
        })
        .detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter branches"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&filter, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let branches = this.visible_branches(cx);
                if this.selected.is_none() && branches.len() == 1 {
                    this.selected = branches.into_iter().next();
                }
                this.submit(cx);
            }
        })
        .detach();
        let handle = owner.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            state,
            repo,
            owner,
            filter,
            selected: None,
        }
    }

    fn fork(&self, cx: &App) -> Option<ForkCheckoutState> {
        self.with_fork(cx, Clone::clone)
    }

    fn with_fork<R>(&self, cx: &App, f: impl FnOnce(&ForkCheckoutState) -> R) -> Option<R> {
        self.state
            .read(cx)
            .repo_states
            .get(&self.repo)
            .and_then(|rs| rs.fork_checkout.as_ref())
            .map(f)
    }

    fn owner_text(&self, cx: &App) -> String {
        self.owner.read(cx).value().trim().to_string()
    }

    fn find(&mut self, cx: &mut Context<Self>) {
        let text = self.owner_text(cx);
        if text.is_empty() {
            return;
        }
        self.selected = None;
        Dispatcher::look_up_fork(self.repo, text, cx);
    }

    /// The found fork's branches matching the filter, when the field still
    /// names that fork.
    fn visible_branches(&self, cx: &App) -> Vec<String> {
        let text = self.owner_text(cx);
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        self.with_fork(cx, |fork| match &fork.lookup {
            ForkLookup::Found(source) if fork.looked_up.trim() == text => source
                .branches
                .iter()
                .filter(|b| filter.is_empty() || b.to_lowercase().contains(&filter))
                .cloned()
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
    }

    fn can_submit(&self, cx: &App) -> bool {
        self.with_fork(cx, |f| !f.checking_out).unwrap_or(false)
            && self
                .selected
                .as_ref()
                .is_some_and(|b| self.visible_branches(cx).contains(b))
    }

    fn submit(&self, cx: &mut App) {
        if !self.can_submit(cx) {
            return;
        }
        if let Some(branch) = self.selected.clone() {
            Dispatcher::checkout_from_fork(self.repo, branch, cx);
        }
    }

    fn row(id: (&'static str, usize), selected: bool, t: &GhdTheme) -> Stateful<Div> {
        let hover = t.box_hover_background;
        div()
            .id(id)
            .h(zpx(29.))
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .px(SPACING())
            .rounded(BORDER_RADIUS())
            .cursor_pointer()
            .text_size(FONT_SIZE())
            .when(selected, |d| {
                d.bg(t.box_selected_background)
                    .text_color(t.box_selected_text)
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!selected, move |d| d.hover(move |s| s.bg(hover)))
    }

    fn list(id: &'static str, rows: Vec<AnyElement>, t: &GhdTheme) -> impl IntoElement {
        div()
            .id(id)
            .h(zpx(210.))
            .overflow_y_scroll()
            .p(SPACING_HALF())
            .flex()
            .flex_col()
            .gap(zpx(1.))
            .border_1()
            .border_color(t.box_border)
            .rounded(BORDER_RADIUS())
            .children(rows)
            .with_scrollbar()
    }

    fn message(text: impl Into<SharedString>, color: Hsla) -> Div {
        div()
            .text_size(FONT_SIZE_SM())
            .text_color(color)
            .child(text.into())
    }
}

impl Render for CheckoutFromForkDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let fork = self.fork(cx).unwrap_or_default();
        let text = self.owner_text(cx);
        let looked_up = fork.looked_up.trim() == text && !text.is_empty();

        let find = button("checkout-from-fork-find", "Find", cx)
            .on_click(cx.listener(|this, _, _, cx| this.find(cx)));
        let owner_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .child(div().flex_1().min_w_0().child(text_box(
                "checkout-from-fork-owner",
                &self.owner,
                None,
                window,
                cx,
            )))
            .child(find);

        let body: AnyElement = if !looked_up {
            // suggestions matching the field
            let needle = text.to_lowercase();
            let rows: Vec<AnyElement> = fork
                .suggestions
                .iter()
                .filter(|s| {
                    needle.is_empty()
                        || s.owner.to_lowercase().contains(&needle)
                        || s.full_name.to_lowercase().contains(&needle)
                })
                .enumerate()
                .map(|(ix, s)| {
                    let owner = s.owner.clone();
                    Self::row(("checkout-from-fork-suggestion", ix), false, &t)
                        .child(octicon(Octicon::RepoForked, t.text_secondary).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(s.full_name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(s.detail.clone()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.owner
                                .update(cx, |input, cx| input.set_value(owner.clone(), window, cx));
                            this.find(cx);
                        }))
                        .into_any_element()
                })
                .collect();
            if rows.is_empty() {
                if fork.suggestions_loading {
                    github_list::loading("forks", "checkout-from-fork-spin", &t).into_any_element()
                } else {
                    Self::message(
                        "Type the owner of the fork and press Find.",
                        t.text_secondary,
                    )
                    .into_any_element()
                }
            } else {
                labeled(
                    "Forks",
                    Self::list("checkout-from-fork-suggestions", rows, &t),
                    cx,
                )
                .into_any_element()
            }
        } else {
            match &fork.lookup {
                ForkLookup::Idle => div().into_any_element(),
                ForkLookup::Loading => {
                    github_list::loading("branches", "checkout-from-fork-spin", &t)
                        .into_any_element()
                }
                ForkLookup::Failed(message) => {
                    Self::message(message.clone(), t.dialog_error).into_any_element()
                }
                ForkLookup::Found(source) => {
                    let branches = self.visible_branches(cx);
                    let hidden = branches.len().saturating_sub(MAX_ROWS);
                    let rows: Vec<AnyElement> = branches
                        .iter()
                        .take(MAX_ROWS)
                        .enumerate()
                        .map(|(ix, b)| {
                            let selected = self.selected.as_deref() == Some(b.as_str());
                            let name = b.clone();
                            Self::row(("checkout-from-fork-branch", ix), selected, &t)
                                .child(octicon(Octicon::GitBranch, t.text_secondary).flex_none())
                                .child(div().flex_1().min_w_0().truncate().child(b.clone()))
                                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                    this.selected = Some(name.clone());
                                    cx.notify();
                                    if ev.click_count() > 1 {
                                        this.submit(cx);
                                    }
                                }))
                                .into_any_element()
                        })
                        .collect();
                    let more = (hidden > 0).then(|| {
                        Self::message(
                            format!("{hidden} more branches; filter to find them."),
                            t.text_secondary,
                        )
                    });
                    let empty = rows.is_empty().then(|| {
                        Self::message(
                            if source.branches.is_empty() {
                                "This fork has no branches."
                            } else {
                                "No branch matches the filter."
                            },
                            t.text_secondary,
                        )
                    });
                    let local = self.selected.as_ref().map(|b| {
                        let s = self.state.read(cx);
                        let names: Vec<&str> = s
                            .repo_states
                            .get(&self.repo)
                            .and_then(|rs| rs.info.as_ref())
                            .map(|info| {
                                info.branches
                                    .iter()
                                    .filter(|b| b.kind == corvene_core::BranchKind::Local)
                                    .map(|b| b.name.as_str())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let name =
                            corvene_core::fork_checkout::fork_branch_name(&source.owner, b, &names);
                        Self::message(
                            format!("Checks out {name}, tracking {b} on {}.", source.label),
                            t.text_secondary,
                        )
                    });
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACING_HALF())
                        .child(labeled(
                            format!("Branches of {}", source.label),
                            filter_text_box(
                                "checkout-from-fork-filter",
                                &self.filter,
                                None,
                                window,
                                cx,
                            ),
                            cx,
                        ))
                        .child(Self::list("checkout-from-fork-branches", rows, &t))
                        .children(more)
                        .children(empty)
                        .children(local)
                        .into_any_element()
                }
            }
        };

        let content = crate::dialog::own_enter(div())
            .w(crate::theme::fit_width(480.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                "Check out a branch from another fork of this repository. A fork without a \
                 remote here gets one named after its owner.",
            )
            .child(labeled("Fork", owner_row, cx))
            .child(body)
            .when_some(fork.error.clone(), |d, error| {
                d.child(Self::message(error, t.dialog_error))
            });
        let weak = cx.weak_entity();
        let can_submit = self.can_submit(cx);
        dialog(
            "checkout-from-fork",
            mac_or("Check Out from Fork", "Check out from fork"),
            content,
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "checkout-from-fork-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "checkout-from-fork-ok",
                    label: if fork.checking_out {
                        "Checking out…".into()
                    } else {
                        mac_or("Check Out Branch", "Check out branch").into()
                    },
                    disabled: !can_submit,
                    on_click: Box::new(move |_, cx| {
                        weak.update(cx, |this, cx| this.submit(cx)).ok();
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
