//! Corvene `1218-compare-refs`: Branch › Compare… (and Compare with… from
//! the History commit menu and the branch list), two pickers that take a
//! branch, a tag or a commit, typed or chosen from a menu of the
//! repository's branches and tags, and whether to list the commits only
//! one side has (`base...head`) instead of those only the second has
//! (`base..head`). A ref git cannot resolve is reported after Compare, by
//! the error dialog. GitHub Desktop has no counterpart.

use corvene_core::{AppState, BranchKind, Dispatcher};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::MenuItem;
use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::icons::{Octicon, octicon};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{IconButtonA11y, checkbox_row, labeled, text_box};
use corvene_core::ref_compare::RefRange;

pub struct CompareRefsDialog {
    state: Entity<AppState>,
    repo: u64,
    base: Entity<InputState>,
    head: Entity<InputState>,
    symmetric: bool,
}

impl CompareRefsDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        base: Option<String>,
        head: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |value: Option<String>, window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder("Branch, tag or commit");
                if let Some(value) = value {
                    input.set_value(value, window, cx);
                }
                input
            });
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.submit(cx);
                }
            })
            .detach();
            input
        };
        // the empty field first
        let focus_head = base.is_some();
        let base = input(base, window, cx);
        let head = input(head, window, cx);
        let focused = if focus_head { &head } else { &base };
        let handle = focused.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        // the picker menus list the tags too
        if state
            .read(cx)
            .repo_states
            .get(&repo)
            .is_some_and(|rs| rs.branch_list_tags.is_none())
        {
            Dispatcher::load_branch_list_tags(repo, cx);
        }
        // `base...head` when the view already shows it
        let symmetric = state
            .read(cx)
            .repo_states
            .get(&repo)
            .and_then(|rs| rs.compare.refs())
            .is_some_and(|(_, _, range)| range == RefRange::Symmetric);
        Self {
            state,
            repo,
            base,
            head,
            symmetric,
        }
    }

    fn values(&self, cx: &App) -> (String, String) {
        (
            self.base.read(cx).value().trim().to_string(),
            self.head.read(cx).value().trim().to_string(),
        )
    }

    fn can_submit(&self, cx: &App) -> bool {
        let (base, head) = self.values(cx);
        !base.is_empty() && !head.is_empty()
    }

    fn submit(&self, cx: &mut App) {
        if !self.can_submit(cx) {
            return;
        }
        let (base, head) = self.values(cx);
        let range = if self.symmetric {
            RefRange::Symmetric
        } else {
            RefRange::Range
        };
        Dispatcher::close_popup(cx);
        Dispatcher::compare_refs(self.repo, base, head, range, false, cx);
    }

    /// The ▾ next to a field: the branches, remote branches and tags.
    fn picker_items(&self, field: Entity<InputState>, cx: &App) -> Vec<MenuItem> {
        let s = self.state.read(cx);
        let Some(rs) = s.repo_states.get(&self.repo) else {
            return Vec::new();
        };
        let pick = |name: String| {
            let field = field.clone();
            MenuItem::new(name.clone(), move |window, cx| {
                field.update(cx, |input, cx| {
                    input.set_value(name.clone(), window, cx);
                    input.focus(window, cx);
                });
            })
        };
        let mut items = Vec::new();
        if let Some(info) = rs.info.as_ref() {
            if let Some(current) = info.current_branch() {
                items.push(pick(current.name.clone()));
                items.push(MenuItem::separator());
            }
            let names = |kind: BranchKind| {
                let mut names: Vec<String> = info
                    .branches
                    .iter()
                    .filter(|b| b.kind == kind)
                    .map(|b| b.name.clone())
                    .collect();
                names.sort_by_key(|n| n.to_lowercase());
                names
            };
            for (label, kind) in [
                ("Branches", BranchKind::Local),
                ("Remote Branches", BranchKind::Remote),
            ] {
                let names = names(kind);
                if !names.is_empty() {
                    items.push(MenuItem::submenu(
                        label,
                        names.into_iter().map(&pick).collect(),
                    ));
                }
            }
        }
        if let Some(tags) = rs.branch_list_tags.as_deref()
            && !tags.is_empty()
        {
            items.push(MenuItem::submenu(
                "Tags",
                tags.iter().map(|(name, _)| pick(name.clone())).collect(),
            ));
        }
        items
    }

    fn field(
        &self,
        id: &'static str,
        label: &'static str,
        input: &Entity<InputState>,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let t = cx.ghd();
        let items = self.picker_items(input.clone(), cx);
        let hover = t.secondary_button_hover_background;
        labeled(
            label,
            div()
                .flex()
                .flex_row()
                .gap(SPACING_HALF())
                .child(div().flex_1().min_w_0().child(text_box(
                    SharedString::from(format!("{id}-text")),
                    input,
                    Some(octicon(Octicon::GitBranch, t.text_secondary)),
                    window,
                    cx,
                )))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-pick")))
                        .icon_button_label(format!("Choose the {}", label.to_lowercase()))
                        .flex_none()
                        .size(TEXT_FIELD_HEIGHT())
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .rounded(BORDER_RADIUS())
                        .border_color(t.box_border_contrast)
                        .bg(t.secondary_button_background)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(move |ev: &ClickEvent, window, cx| {
                            let position = ev.mouse_position().unwrap_or_default();
                            crate::native_menu::show_context_menu(
                                items.clone(),
                                position,
                                window,
                                cx,
                            );
                        })
                        .child(octicon(Octicon::TriangleDown, t.text)),
                ),
            cx,
        )
    }
}

impl Render for CompareRefsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let this = cx.entity().downgrade();
        let submit = cx.entity().downgrade();
        let symmetric = self.symmetric;
        let disabled = !self.can_submit(cx);
        let (base, head) = self.values(cx);
        let separator = if symmetric { "..." } else { ".." };
        let hint = if base.is_empty() || head.is_empty() {
            String::new()
        } else if symmetric {
            format!("Commits only one of them has: {base}{separator}{head}")
        } else {
            format!("Commits {head} has and {base} has not: {base}{separator}{head}")
        };
        let t = cx.ghd().clone();
        dialog(
            "compare-refs",
            "Compare",
            crate::dialog::own_enter(div())
                .w(crate::theme::fit_width(420.))
                .flex()
                .flex_col()
                .gap(SPACING())
                .child(self.field("compare-refs-base", "Base", &self.base, window, cx))
                .child(self.field("compare-refs-head", "Compare", &self.head, window, cx))
                .child(checkbox_row(
                    "compare-refs-symmetric",
                    symmetric,
                    "Also list the commits only the base has (...)",
                    move |checked, _, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.symmetric = checked;
                            cx.notify();
                        });
                    },
                    cx,
                ))
                .child(
                    div()
                        .min_h(zpx(16.))
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.text_secondary)
                        .child(hint),
                ),
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "compare-refs-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "compare-refs-ok",
                    label: "Compare".into(),
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        let _ = submit.update(cx, |this, cx| this.submit(cx));
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
