//! "Add Local Repository" (`ui/add-repository/add-existing-repository.tsx`).
//! Like GHD 3.6.6 the path is only checked on submit (`addRepository` →
//! `validatePath`), the warning then staying until the next check - unless
//! flag `205-add-local-validates-while-typing` checks it on every change and
//! keeps Add Repository disabled until the path is a repository.
//!
//! Deviation (`222-add-local-multiple`): Choose… can pick several folders;
//! more than one adds every picked repository at once.
//!
//! Deviation (`224-alias-when-adding`): an optional Alias field names the
//! repository as it is added (GHD: Create Alias afterwards).
//!
//! Deviation (`223-add-local-path-completion`): the Local Path box
//! autocompletes folder names (↑/↓, Enter/Tab, Esc) like the Add Worktree
//! branch box.
//!
//! Deviation (`289-add-repositories-in-folder`): a folder that is not a
//! repository but holds some (two levels down) offers "N repositories found
//! inside. Add them all" under the not-a-repository warning.
//!
//! A path in double quotes (pasted from Explorer's Copy as Path) is read
//! without them, as GHD does since desktop/desktop#23108 (after 3.6.6).

use std::path::PathBuf;
use std::rc::Rc;

use corvene_core::{AppState, Dispatcher, Popup};
use corvene_git::PathStatus;
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InputEvent, InputState, MoveDown, MoveUp,
};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::autocompletion::{self, Autocompletion, PickHandler};
use crate::context_menu::mac_or;
use crate::dialog::{GroupButtonSpec, OkCancelButtonGroup, dialog};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{button, labeled, text_box};

pub struct AddExistingRepositoryDialog {
    state: Entity<AppState>,
    path: Entity<InputState>,
    /// The last `validatePath` result that warrants a warning
    /// (`showNonGitRepositoryWarning` / `isRepositoryBare`).
    warning: Option<PathStatus>,
    /// `223-add-local-path-completion` popup.
    autocomplete: Option<Autocompletion>,
    /// `224-alias-when-adding`.
    alias: Entity<InputState>,
    /// `289-add-repositories-in-folder`: the last folder scanned and the
    /// repositories found in it (`None` while the scan runs).
    inside: Option<(PathBuf, Option<Vec<PathBuf>>)>,
}

impl AddExistingRepositoryDialog {
    pub fn new(
        state: Entity<AppState>,
        initial: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("repository path");
            if let Some(p) = &initial {
                s = s.default_value(p.display().to_string());
            }
            s
        });
        cx.observe(&path, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&path, |this, _, ev: &InputEvent, cx| match ev {
            InputEvent::Change => this.open_autocomplete(cx),
            InputEvent::Blur => {
                this.autocomplete = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        let handle = path.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        Self {
            state,
            path,
            warning: None,
            autocomplete: None,
            alias: cx.new(|cx| InputState::new(window, cx).placeholder("optional")),
            inside: None,
        }
    }

    /// `289-add-repositories-in-folder`: the repositories inside `path`,
    /// scanned once per path on a background thread (`None` until known).
    fn repositories_inside(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<Vec<PathBuf>> {
        if let Some((scanned, found)) = &self.inside
            && *scanned == path
        {
            return found.clone();
        }
        self.inside = Some((path.clone(), None));
        let scan = path.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { corvene_git::repositories_inside(&scan, 2) })
                .await;
            this.update(cx, |d, cx| {
                if d.inside.as_ref().is_some_and(|(p, _)| *p == path) {
                    d.inside = Some((path, Some(found)));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        None
    }

    fn open_autocomplete(&mut self, cx: &mut Context<Self>) {
        let on = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::ADD_LOCAL_PATH_COMPLETION);
        let text = self.path.read(cx).value().to_string();
        self.autocomplete = on.then(|| autocompletion::attempt_path(&text)).flatten();
        cx.notify();
    }

    fn autocomplete_move(&mut self, delta: i64, cx: &mut Context<Self>) -> bool {
        match self.autocomplete.as_mut() {
            Some(ac) => {
                ac.move_selection(delta);
                cx.notify();
                true
            }
            None => false,
        }
    }

    fn autocomplete_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.autocomplete.as_ref().and_then(|ac| ac.selected) {
            Some(ix) => {
                self.autocomplete_insert(ix, window, cx);
                true
            }
            None => false,
        }
    }

    /// The folder's path replaces the text; its own sub-folders are then
    /// offered.
    fn autocomplete_insert(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self
            .autocomplete
            .take()
            .and_then(|ac| ac.hits.get(ix).cloned())
        else {
            return;
        };
        self.path
            .update(cx, |s, cx| s.set_value(hit.completion_text(), window, cx));
        let handle = self.path.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn resolved_path(&self, cx: &App) -> Option<PathBuf> {
        let raw = self.path.read(cx).value();
        let raw = unquote(raw.trim());
        if raw.is_empty() {
            return None;
        }
        let home_relative = raw
            .strip_prefix("~/")
            .or_else(|| raw.strip_prefix("~\\").filter(|_| cfg!(windows)));
        let expanded = if let Some(rest) = home_relative {
            dirs_home().join(rest)
        } else {
            PathBuf::from(raw)
        };
        Some(expanded)
    }

    fn status(&self, cx: &App) -> Option<PathStatus> {
        self.resolved_path(cx).map(|p| corvene_git::path_status(&p))
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let multiple = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::ADD_LOCAL_MULTIPLE);
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple,
            prompt: Some(mac_or("Add Repository", "Add repository").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            if paths.len() > 1 {
                cx.update(|_, cx| add_several(paths, cx)).ok();
            } else if let Some(p) = paths.into_iter().next() {
                this.update_in(cx, |d, window, cx| {
                    d.path
                        .update(cx, |s, cx| s.set_value(p.display().to_string(), window, cx));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let status = self.status(cx);
        match (self.resolved_path(cx), status) {
            (Some(path), Some(PathStatus::Repository)) => {
                // a subdirectory adds its repository (`Dispatcher::add_repository`
                // resolves the top level, as GHD `_addRepositories`)
                Dispatcher::close_popup(cx);
                if self
                    .state
                    .read(cx)
                    .flags
                    .bool(corvene_core::flags::ids::ALIAS_WHEN_ADDING)
                {
                    let alias = self.alias.read(cx).value().to_string();
                    Dispatcher::alias_when_added(&path, alias, cx);
                }
                Dispatcher::add_repository(path, cx);
            }
            (_, status) => {
                self.warning =
                    status.filter(|s| matches!(s, PathStatus::NotARepository | PathStatus::Bare));
                cx.notify();
            }
        }
    }
}

/// `222-add-local-multiple`: add every picked folder that is a repository
/// and name the ones that are not.
fn add_several(paths: Vec<PathBuf>, cx: &mut App) {
    let (repos, others): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .into_iter()
        .partition(|p| corvene_git::path_status(p) == PathStatus::Repository);
    Dispatcher::close_popup(cx);
    for path in repos {
        Dispatcher::add_repository(path, cx);
    }
    if !others.is_empty() {
        let names: Vec<String> = others.iter().map(|p| p.display().to_string()).collect();
        Dispatcher::show_error(
            "Some folders were not added",
            format!(
                "These folders do not appear to be Git repositories:\n{}",
                names.join("\n")
            ),
            cx,
        );
    }
}

/// GHD `unquotePath`: one pair of surrounding double quotes is dropped.
fn unquote(path: &str) -> &str {
    path.strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .unwrap_or(path)
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl Render for AddExistingRepositoryDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let has_path = self.resolved_path(cx).is_some();
        let live = corvene_core::AppState::global(cx)
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::ADD_LOCAL_VALIDATES_WHILE_TYPING);
        let current = live.then(|| self.status(cx)).flatten();
        let status = if live {
            current.filter(|s| matches!(s, PathStatus::NotARepository | PathStatus::Bare))
        } else {
            self.warning.filter(|_| has_path)
        };
        let add_disabled = live && current != Some(PathStatus::Repository);
        let error: Option<AnyElement> = match status {
            // `buildNotAGitRepositoryError`: two paragraphs, the second
            // linking "create a repository"
            Some(PathStatus::NotARepository) => Some({
                let path = self.resolved_path(cx);
                // Corvene (`289-add-repositories-in-folder`)
                let inside = path
                    .clone()
                    .filter(|_| {
                        self.state
                            .read(cx)
                            .flags
                            .bool(corvene_core::flags::ids::ADD_REPOSITORIES_IN_FOLDER)
                    })
                    .and_then(|p| self.repositories_inside(p, cx))
                    .filter(|found| !found.is_empty());
                div()
                    .flex()
                    .flex_col()
                    .text_color(t.error)
                    .child("This directory does not appear to be a Git repository.")
                    .child(crate::widgets::paragraph(vec![
                        "Would you like to ".into(),
                        crate::widgets::link_button("create-instead", "create a repository", cx)
                            .on_click(move |_, _, cx| {
                                // GHD `onCreateRepositoryClicked`: this
                                // dialog closes first
                                Dispatcher::close_popup(cx);
                                Dispatcher::show_popup(
                                    Popup::CreateRepository { path: path.clone() },
                                    cx,
                                )
                            })
                            .into_any_element()
                            .into(),
                        " here instead?".into(),
                    ]))
                    .when_some(inside, |d, found| {
                        let (count, add) = if found.len() == 1 {
                            ("1 repository found inside. ".to_string(), "Add it")
                        } else {
                            (
                                format!("{} repositories found inside. ", found.len()),
                                "Add them all",
                            )
                        };
                        d.child(crate::widgets::paragraph(vec![
                            div().text_color(t.text).child(count).into_any_element().into(),
                            crate::widgets::link_button("add-inside", add, cx)
                                .on_click(move |_, _, cx| add_several(found.clone(), cx))
                                .into_any_element()
                                .into(),
                        ]))
                    })
                    .into_any_element()
            }),
            Some(PathStatus::Bare) => Some(
                div()
                    .text_color(t.error)
                    .child("This directory appears to be a bare repository. Bare repositories are not currently supported.")
                    .into_any_element(),
            ),
            _ => None,
        };

        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let this = cx.entity();
        let alias_field = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::ALIAS_WHEN_ADDING);
        let popup = self.autocomplete.as_ref().and_then(|ac| {
            let (bounds, line_height) = self.path.read(cx).cursor_layout()?;
            let anchor = point(bounds.origin.x, bounds.origin.y + line_height);
            let weak = cx.weak_entity();
            let on_pick: PickHandler = Rc::new(move |ix, window, cx| {
                weak.update(cx, |this, cx| this.autocomplete_insert(ix, window, cx))
                    .ok();
            });
            Some(autocompletion::dialog_popup(ac, anchor, on_pick, cx))
        });
        dialog(
            "add-existing-repository",
            mac_or("Add Local Repository", "Add local repository"),
            div()
                .flex()
                .flex_col()
                .gap(SPACING())
                .child(
                    // `Row`: [Local Path text box][Choose…]; the popup's keys
                    // win over the field's own bindings
                    div()
                        .key_context("AutocompletingTextInput")
                        .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                            if this.autocomplete_move(-1, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                            if this.autocomplete_move(1, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(|this, _: &Enter, window, cx| {
                            if this.autocomplete_accept(window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                            if this.autocomplete_accept(window, cx) {
                                cx.stop_propagation();
                            }
                        }))
                        .capture_action(cx.listener(
                            |this, _: &crate::actions::FocusNext, window, cx| {
                                if this.autocomplete_accept(window, cx) {
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                            if this.autocomplete.take().is_some() {
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .children(popup)
                        .flex()
                        .flex_row()
                        .items_end()
                        .gap(SPACING())
                        .child(labeled(
                            mac_or("Local Path", "Local path"),
                            text_box("add-existing-path", &self.path, None, window, cx),
                            cx,
                        ))
                        .child(
                            button("add-existing-choose", "Choose…", cx)
                                .flex_none()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.choose(window, cx)),
                                ),
                        ),
                )
                .children(crate::widgets::android_storage_note(
                    crate::widgets::StorageNote::AddLocal,
                    cx,
                ))
                .children(error)
                .when(alias_field, |d| {
                    d.child(labeled(
                        "Alias",
                        text_box("add-existing-alias", &self.alias, None, window, cx),
                        cx,
                    ))
                }),
            OkCancelButtonGroup {
                destructive: false,
                cancel: GroupButtonSpec {
                    id: "add-existing-cancel",
                    label: "Cancel".into(),
                    disabled: false,
                    on_click: Box::new(close),
                },
                ok: GroupButtonSpec {
                    id: "add-existing-ok",
                    label: mac_or("Add Repository", "Add repository").into(),
                    disabled: add_disabled,
                    on_click: Box::new(move |_, cx| {
                        this.update(cx, |d, cx| d.submit(cx));
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

#[cfg(test)]
mod tests {
    use super::unquote;

    #[test]
    fn quoted_paths_lose_one_pair_of_quotes() {
        assert_eq!(unquote("\"/tmp/my repo\""), "/tmp/my repo");
        assert_eq!(unquote("/tmp/repo"), "/tmp/repo");
        assert_eq!(unquote("\"/tmp/repo"), "\"/tmp/repo");
        assert_eq!(unquote("\""), "\"");
    }
}
