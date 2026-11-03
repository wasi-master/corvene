//! Corvene addition (`1313-changelists`, desktop/desktop#16925): New
//! Changelist… and Edit Changelist…. GHD has no changelists
//! (`ui/changes/filter-changes-list.tsx` lists changed files flat).

use corvene_core::{AppState, Dispatcher};
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::context_menu::mac_or;
use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{checkbox_row, input_error, labeled, text_box};

/// A list's name and description; `paths` move into a new list, `edit`
/// names the list being edited.
pub struct NewChangelistDialog {
    state: Entity<AppState>,
    repo: u64,
    paths: Vec<String>,
    edit: Option<u64>,
    name: Entity<InputState>,
    description: Entity<TextareaState>,
    /// Make it the active list (new changes join it, only it is committed).
    active: bool,
}

impl NewChangelistDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        paths: Vec<String>,
        edit: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let existing = edit.and_then(|id| {
            let s = state.read(cx);
            let lists = s.changelists.get(&repo)?;
            let list = lists.get(id)?;
            Some((
                list.name.clone(),
                list.description.clone(),
                lists.active == Some(id),
            ))
        });
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Name (required)");
            if let Some((name, _, _)) = &existing {
                input.set_value(name.clone(), window, cx);
            }
            input
        });
        let description = cx.new(|cx| {
            let mut area = TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Description (optional, used as the commit message)");
            if let Some((_, description, _)) = &existing {
                area.set_value(description.clone(), window, cx);
            }
            area
        });
        cx.observe(&name, |_, _, cx| cx.notify()).detach();
        cx.observe(&description, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&name, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit(cx);
            }
        })
        .detach();
        let handle = name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            state,
            repo,
            paths,
            edit,
            name,
            description,
            active: existing.as_ref().is_some_and(|(_, _, active)| *active),
        }
    }

    /// The trimmed name, and whether another list has it.
    fn name_state(&self, cx: &App) -> (String, bool) {
        let name = self.name.read(cx).value().trim().to_string();
        let taken = self
            .state
            .read(cx)
            .changelists
            .get(&self.repo)
            .is_some_and(|lists| lists.name_taken(&name, self.edit));
        (name, taken)
    }

    fn submit(&self, cx: &mut App) {
        let (name, taken) = self.name_state(cx);
        if name.is_empty() || taken {
            return;
        }
        let description = self.description.read(cx).value().trim().to_string();
        Dispatcher::close_popup(cx);
        match self.edit {
            Some(id) => {
                Dispatcher::update_changelist(self.repo, id, name, description, cx);
                let was_active = self
                    .state
                    .read(cx)
                    .changelists
                    .get(&self.repo)
                    .is_some_and(|l| l.active == Some(id));
                if was_active != self.active {
                    Dispatcher::set_active_changelist(self.repo, self.active.then_some(id), cx);
                }
            }
            None => Dispatcher::create_changelist(
                self.repo,
                name,
                description,
                self.paths.clone(),
                self.active,
                cx,
            ),
        }
    }
}

impl Render for NewChangelistDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let t = cx.ghd();
        let (name, taken) = self.name_state(cx);
        let editing = self.edit.is_some();
        let this = cx.entity().downgrade();
        let submit = cx.entity().downgrade();
        let files_note = (!editing && !self.paths.is_empty()).then(|| {
            let n = self.paths.len();
            div()
                .text_color(t.text_secondary)
                .text_size(FONT_SIZE_SM())
                .child(if n == 1 {
                    format!("{} moves into the new changelist.", self.paths[0])
                } else {
                    format!("{n} files move into the new changelist.")
                })
        });
        let notice = taken.then(|| {
            input_error(format!("A changelist named {name} already exists."), cx).into_any_element()
        });
        dialog(
            "new-changelist",
            if editing {
                mac_or("Edit Changelist", "Edit changelist")
            } else {
                mac_or("New Changelist", "New changelist")
            },
            crate::dialog::own_enter(div())
                .w(crate::theme::fit_width(400.))
                .flex()
                .flex_col()
                .gap(SPACING())
                .child(labeled(
                    "Name",
                    text_box("new-changelist-name", &self.name, None, window, cx),
                    cx,
                ))
                .children(notice)
                .child(labeled(
                    "Description",
                    div()
                        .border_1()
                        .border_color(t.box_border_contrast)
                        .rounded(BORDER_RADIUS())
                        .bg(t.box_background)
                        .overflow_hidden()
                        .child(crate::widgets::resizable_text_area(
                            "new-changelist-description",
                            Textarea::new(&self.description),
                            cx,
                        )),
                    cx,
                ))
                .child(checkbox_row(
                    "new-changelist-active",
                    self.active,
                    "Make this the active changelist (new changes join it, and only its \
                     files are committed)",
                    move |checked, _, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.active = checked;
                            cx.notify();
                        });
                    },
                    cx,
                ))
                .children(files_note),
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "new-changelist-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "new-changelist-ok",
                    label: if editing {
                        "Save".into()
                    } else {
                        mac_or("Create Changelist", "Create changelist").into()
                    },
                    disabled: name.is_empty() || taken,
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
