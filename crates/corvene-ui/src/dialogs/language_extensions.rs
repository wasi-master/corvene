//! Language Extensions (Corvene addition, no GitHub Desktop counterpart;
//! flag `111-language-extensions`): the grammars the user installed from
//! editor extensions (`corvene_core::extensions`). Opened from Settings ›
//! Appearance › Language extensions… (and `CORVENE_POPUP=language-extensions`).
//!
//! Three tabs. Installed: one row per extension with its source, the file
//! types it covers, its state and the Enabled / Prefer switches, the
//! selected one's details on the right (including what its grammars lost
//! in conversion); the footer adds a file or folder, a URL or a GitHub
//! repository. Find: a search over the Open VSX, Zed and Pulsar registries
//! by name or by file suffix (`.foo`). Import: the grammar extensions of
//! the editors installed on this machine, copied in without a download.
//! Snapshot-verified only: the parity harness has nothing to compare it
//! with.

use std::collections::BTreeSet;
use std::path::PathBuf;

use corvene_core::extensions::{
    ExtensionProgress, ExtensionsFocus, ExtensionsState, InstallSource,
};
use corvene_core::{AppState, Dispatcher};
use corvene_extensions::github::RepoRef;
use corvene_extensions::importer::{Editor, ImportCandidate};
use corvene_extensions::install::{self, GrammarKind, Installed, Resolution, SourceKind, Status};
use corvene_extensions::registry::{self, Candidate, GrammarHint, Registry};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::dialog::{DialogFrame, dialog_with_frame};
use crate::icons::{Octicon, octicon};
use crate::scrollbar::ScrollbarExt;
use crate::tab_bar::{TabModel, tab_bar};
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{
    button, checkbox, filter_text_box, input_error, link_button, pill, primary_button,
    settings_description, small_button, switch, text_box,
};

/// The dialog's width (CSS px).
const WIDTH: f32 = 760.;
/// The list and details area's height.
const HEIGHT: f32 = 440.;
/// The details column's width.
const DETAILS_WIDTH: f32 = 280.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Installed,
    Find,
    Import,
}

/// The inline input under the Installed list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddMode {
    Url,
    GitHub,
}

pub struct LanguageExtensionsDialog {
    state: Entity<AppState>,
    tab: Tab,
    filter: Entity<InputState>,
    query: Entity<InputState>,
    add_input: Entity<InputState>,
    add_mode: Option<AddMode>,
    add_error: Option<String>,
    import_checked: BTreeSet<PathBuf>,
    /// the consent sheet's "Remember for this grammar version"
    remember_consent: bool,
}

impl LanguageExtensionsDialog {
    pub fn new(
        state: Entity<AppState>,
        focus: Option<ExtensionsFocus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter installed"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        let initial_query = match &focus {
            Some(ExtensionsFocus::Suffix(suffix)) => format!(".{}", suffix.trim_start_matches('.')),
            _ => String::new(),
        };
        let query = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("Search by name, or by file suffix like .nix");
            if !initial_query.is_empty() {
                s = s.default_value(initial_query.clone());
            }
            s
        });
        cx.observe(&query, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&query, |this, input, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let text = input.read(cx).value().to_string();
                this.search(text, cx);
            }
        })
        .detach();
        let add_input = cx.new(|cx| InputState::new(window, cx));
        cx.observe(&add_input, |this, _, cx| {
            this.add_error = None;
            cx.notify();
        })
        .detach();
        cx.subscribe(&add_input, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit_add(cx);
            }
        })
        .detach();
        let tab = match &focus {
            Some(
                ExtensionsFocus::Suffix(_) | ExtensionsFocus::Find | ExtensionsFocus::IconThemes,
            ) => Tab::Find,
            Some(ExtensionsFocus::Import) => Tab::Import,
            None => Tab::Installed,
        };
        // `117-file-icons`: the Find tab lists icon themes when asked for them
        let icon_themes = focus == Some(ExtensionsFocus::IconThemes);
        if icon_themes || state.read(cx).extensions.search.icon_themes {
            Dispatcher::set_extension_search_icon_themes(icon_themes, icon_themes, cx);
        }
        let mut this = Self {
            state,
            tab,
            filter,
            query,
            add_input,
            add_mode: None,
            add_error: None,
            import_checked: BTreeSet::new(),
            remember_consent: false,
        };
        if !initial_query.is_empty() {
            this.search(initial_query, cx);
        }
        if tab == Tab::Import {
            Dispatcher::scan_installed_editors(cx);
        }
        Dispatcher::check_extension_updates(cx);
        this
    }

    fn search(&mut self, text: String, cx: &mut Context<Self>) {
        self.tab = Tab::Find;
        Dispatcher::search_extensions(text, cx);
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Import && self.state.read(cx).extensions.import_candidates.is_none() {
            Dispatcher::scan_installed_editors(cx);
        }
        cx.notify();
    }

    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                cx.update(|_, cx| {
                    for path in paths {
                        Dispatcher::install_extension(InstallSource::LocalPath(path), cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn open_add(&mut self, mode: AddMode, window: &mut Window, cx: &mut Context<Self>) {
        self.add_mode = Some(mode);
        self.add_error = None;
        let placeholder = match mode {
            AddMode::Url => "https://… (a .vsix, .zip, .tar.gz or grammar file)",
            AddMode::GitHub => "owner/repository, owner/repository@branch, or a github.com link",
        };
        self.add_input.update(cx, |s, cx| {
            s.set_placeholder(placeholder, window, cx);
            s.set_value("", window, cx);
        });
        cx.notify();
    }

    fn submit_add(&mut self, cx: &mut Context<Self>) {
        let Some(mode) = self.add_mode else {
            return;
        };
        let text = self.add_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let source = match mode {
            AddMode::Url => {
                if !text.starts_with("https://") {
                    self.add_error = Some("Only https:// addresses are downloaded.".to_string());
                    cx.notify();
                    return;
                }
                InstallSource::Url(text)
            }
            AddMode::GitHub => match RepoRef::parse(&text) {
                Ok(repo) => InstallSource::GitHub(repo),
                Err(err) => {
                    self.add_error = Some(err.to_string());
                    cx.notify();
                    return;
                }
            },
        };
        self.add_mode = None;
        self.add_error = None;
        Dispatcher::install_extension(source, cx);
        cx.notify();
    }

    /// The installed extensions matching the filter box.
    fn visible(&self, extensions: &ExtensionsState, cx: &App) -> Vec<Installed> {
        let needle = self.filter.read(cx).value().trim().to_lowercase();
        extensions
            .installed
            .iter()
            .filter(|i| {
                needle.is_empty()
                    || i.metadata.display_name.to_lowercase().contains(&needle)
                    || i.metadata.suffixes().iter().any(|s| s.contains(&needle))
                    || i.metadata.languages.iter().any(|l| {
                        l.name
                            .as_deref()
                            .is_some_and(|n| n.to_lowercase().contains(&needle))
                    })
            })
            .cloned()
            .collect()
    }

    // ---- Installed ----

    fn row(
        &self,
        installed: &Installed,
        extensions: &ExtensionsState,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let md = &installed.metadata;
        let id = md.id.clone();
        let selected = extensions.selected.as_deref() == Some(md.id.as_str());
        let (dot, state_text) = extension_state(installed, extensions);
        let suffixes: Vec<String> = md.suffixes().iter().map(|s| format!(".{s}")).collect();
        let mut file_types = suffixes.join(" ");
        let filenames: Vec<&str> = md
            .languages
            .iter()
            .flat_map(|l| l.filenames.iter().map(String::as_str))
            .collect();
        if !filenames.is_empty() {
            if !file_types.is_empty() {
                file_types.push_str("  ");
            }
            file_types.push_str(&filenames.join(" "));
        }
        // `117-file-icons`
        let icon_themes: Vec<&str> = md.icon_themes.iter().map(|t| t.label.as_str()).collect();
        if !icon_themes.is_empty() {
            if !file_types.is_empty() {
                file_types.push_str("  ·  ");
            }
            file_types.push_str(&format!("File icons: {}", icon_themes.join(", ")));
        }
        if file_types.is_empty() {
            file_types = "no file types".to_string();
        }
        let kind = if md.grammars.is_empty() && !icon_themes.is_empty() {
            "icon theme"
        } else {
            grammar_kind_label(installed)
        };
        let error = extensions.errors.get(&md.id).cloned();
        let update = extensions.updates.get(&md.id).cloned();
        let build_flag = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::BUILD_GRAMMARS_FROM_SOURCE);
        // tree-sitter grammars waiting for a build, and builds in flight
        let needs_build: Vec<String> = md
            .grammars
            .iter()
            .filter(|g| g.resolution == Resolution::NeedsBuild && g.repository.is_some())
            .map(|g| g.name.clone())
            .collect();
        let building: Vec<String> = md
            .grammars
            .iter()
            .filter_map(|g| {
                extensions
                    .builds
                    .get(&format!("{}/{}", md.id, g.name))
                    .map(|b| format!("{}: {}", g.name, b.stage.describe()))
            })
            .collect();
        let build_errors: Vec<String> = md
            .grammars
            .iter()
            .filter_map(|g| {
                extensions
                    .build_errors
                    .get(&format!("{}/{}", md.id, g.name))
                    .map(|e| format!("{}: {e}", g.name))
            })
            .collect();
        let row_id = SharedString::from(format!("lang-ext-row-{}", md.id));
        let select_id = id.clone();
        let enabled_id = id.clone();
        let prefer_id = id.clone();
        let prefer_now = md.prefer_over_builtin;
        let remove_id = id.clone();
        let (bg, hover) = if selected {
            (t.box_selected_background, t.box_selected_background)
        } else {
            (t.background, t.list_item_hover_background)
        };
        div()
            .id(row_id)
            .flex()
            .flex_col()
            .px(SPACING())
            .py(SPACING_HALF())
            .gap(zpx(3.))
            .bg(bg)
            .hover(move |s| s.bg(hover))
            .border_b_1()
            .border_color(t.box_border)
            .cursor_pointer()
            .on_click(move |_, _, cx| Dispatcher::select_extension(Some(select_id.clone()), cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(div().size(zpx(8.)).rounded_full().flex_none().bg(dot))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .min_w_0()
                            .truncate()
                            .child(md.display_name.clone()),
                    )
                    .child(pill(
                        md.source.kind.title(),
                        None,
                        t.list_item_badge_background,
                        t.list_item_badge_text,
                        cx,
                    ))
                    .children(md.version.clone().map(|v| {
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .child(v)
                    }))
                    .child(div().flex_1())
                    .children(needs_build.first().filter(|_| build_flag && building.is_empty()).map(|grammar| {
                        let (build_id, build_grammar) = (id.clone(), grammar.clone());
                        small_button(
                            SharedString::from(format!("lang-ext-build-{}", md.id)),
                            if needs_build.len() == 1 { "Build Grammar…".to_string() } else { format!("Build {} Grammars…", needs_build.len()) },
                            cx,
                        )
                        .on_click(move |_, _, cx| Dispatcher::request_grammar_build(&build_id, &build_grammar, cx))
                    }))
                    .children(update.map(|candidate| {
                        let label = format!(
                            "Update to {}",
                            candidate.version.clone().unwrap_or_default()
                        );
                        small_button(SharedString::from(format!("lang-ext-update-{}", md.id)), label, cx)
                            .on_click(move |_, _, cx| {
                                Dispatcher::install_extension(InstallSource::Registry(candidate.clone()), cx)
                            })
                    })),
            )
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .min_w_0()
                    .truncate()
                    .child(format!("{file_types}  ·  {kind}  ·  {state_text}")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .child(switch(
                                SharedString::from(format!("lang-ext-enabled-{}", md.id)),
                                md.enabled,
                                false,
                                move |on, _, cx| Dispatcher::set_extension_enabled(&enabled_id, on, cx),
                                cx,
                            ))
                            .child(div().text_size(FONT_SIZE_SM()).child("Enabled")),
                    )
                    // `117-file-icons`: nothing to prefer without a grammar
                    .when(!md.grammars.is_empty(), |d| d.child(
                        div()
                            .id(SharedString::from(format!("lang-ext-prefer-row-{}", md.id)))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .cursor_pointer()
                            .child(checkbox(
                                SharedString::from(format!("lang-ext-prefer-{}", md.id)),
                                md.prefer_over_builtin,
                                false,
                                cx,
                            ))
                            .child(
                                div()
                                    .text_size(FONT_SIZE_SM())
                                    .child("Prefer over built-in highlighting"),
                            )
                            .on_click(move |_, _, cx| {
                                Dispatcher::set_extension_preferred(&prefer_id, !prefer_now, cx)
                            }),
                    ))
                    .child(div().flex_1())
                    .child(
                        link_button(SharedString::from(format!("lang-ext-remove-{}", md.id)), "Remove", cx)
                            .text_size(FONT_SIZE_SM())
                            .on_click(move |_, _, cx| Dispatcher::remove_extension(&remove_id, cx)),
                    ),
            )
            .children(building.into_iter().map(|b| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(b)
            }))
            .children(build_errors.into_iter().chain(error).map(|e| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.color_deleted)
                    .whitespace_normal()
                    .child(e)
            }))
            .when(!needs_build.is_empty() && !build_flag, |d| {
                d.child(
                    div()
                        .text_size(FONT_SIZE_SM())
                        .text_color(t.color_modified)
                        .whitespace_normal()
                        .child("Its tree-sitter grammar is not bundled with Corvene; the flag build-grammars-from-source offers to compile it."),
                )
            })
            .into_any_element()
    }

    /// The consent sheet over the dialog: what a grammar build will
    /// download and run, before anything happens.
    fn consent_sheet(
        &self,
        consent: &corvene_core::extensions::BuildConsent,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let plan = &consent.plan;
        let line = |text: String| {
            div()
                .text_size(FONT_SIZE_SM())
                .whitespace_normal()
                .min_w_0()
                .child(text)
        };
        let remember = self.remember_consent;
        let can_build = consent.compiler.is_ok();
        let compiler_line = match &consent.compiler {
            Ok(compiler) => line(format!(
                "It will compile the parser with the compiler on this Mac: {} ({}).",
                compiler.cc.display(),
                compiler.version
            )),
            Err(message) => line(message.clone()).text_color(t.color_deleted),
        };
        let card = div()
            .w(zpx(520.))
            .max_w_full()
            .flex()
            .flex_col()
            .gap(SPACING())
            .p(SPACING_DOUBLE())
            .rounded(zpx(6.))
            .bg(t.background)
            .border_1()
            .border_color(t.box_border)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(octicon(Octicon::Alert, t.color_modified).size(zpx(16.)))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("Build the {} grammar from source?", consent.grammar))),
            )
            .child(line(format!(
                "Corvene will download the grammar's source code from {} at commit {} ({}).",
                plan.repository,
                plan.short_rev(),
                plan.tarball_url
            )))
            .child(compiler_line)
            .child(line(format!(
                "The built library is kept in {} and loaded into Corvene the next time a diff needs it.",
                plan.library_path().parent().map(|p| p.display().to_string()).unwrap_or_default()
            )))
            .child(
                line("A grammar's code runs inside Corvene; a bug in it can crash the app. Only build grammars from sources you trust.".to_string())
                    .text_color(t.text_secondary),
            )
            .child(
                div()
                    .id("lang-ext-consent-remember-row")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .child(checkbox("lang-ext-consent-remember", remember, !can_build, cx))
                    .child(div().text_size(FONT_SIZE_SM()).child("Remember for this grammar version"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.remember_consent = !this.remember_consent;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(SPACING())
                    .child(
                        button("lang-ext-consent-cancel", "Cancel", cx)
                            .on_click(|_, _, cx| Dispatcher::respond_to_build_consent(false, false, cx)),
                    )
                    .child(
                        primary_button("lang-ext-consent-build", "Download and Build", !can_build, cx).on_click(
                            cx.listener(|this, _, _, cx| {
                                let remember = this.remember_consent;
                                this.remember_consent = false;
                                Dispatcher::respond_to_build_consent(true, remember, cx);
                            }),
                        ),
                    ),
            );
        div()
            .id("lang-ext-consent")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.dialog_backdrop)
            .occlude()
            .child(card)
            .into_any_element()
    }

    fn pending_row(
        &self,
        key: &str,
        progress: &ExtensionProgress,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let what = key
            .strip_prefix("pending:")
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| "extension".to_string());
        div()
            .flex()
            .flex_col()
            .px(SPACING())
            .py(SPACING_HALF())
            .gap(zpx(3.))
            .border_b_1()
            .border_color(t.box_border)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Installing {what}…")),
            )
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(progress.describe()),
            )
            .into_any_element()
    }

    fn details(&self, extensions: &ExtensionsState, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let Some(installed) = extensions
            .selected
            .as_deref()
            .and_then(|id| extensions.get(id))
        else {
            let text = if extensions.installed.is_empty() {
                "Add an extension to highlight more languages: a .vsix or .zip from VS Code or \
                 Open VSX, a Zed or Pulsar extension, a Sublime Text package, a TextMate bundle \
                 or a single grammar file. Find searches the registries; Import takes grammars \
                 from the editors on this Mac."
            } else {
                "Select an extension to see what it provides."
            };
            return div()
                .p(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .whitespace_normal()
                .child(text)
                .into_any_element();
        };
        let md = &installed.metadata;
        let heading = |text: &str| {
            div()
                .mt(SPACING())
                .text_size(FONT_SIZE_SM())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_secondary)
                .child(text.to_string())
        };
        let line = |text: String| {
            div()
                .text_size(FONT_SIZE_SM())
                .min_w_0()
                .whitespace_normal()
                .child(text)
        };
        let source = match (&md.source.url, &md.source.path, &md.source.editor) {
            (Some(url), _, _) => format!("{} · {url}", md.source.kind.title()),
            (_, Some(path), Some(editor)) => format!("{editor} · {path}"),
            (_, Some(path), None) => format!("{} · {path}", md.source.kind.title()),
            _ => md.source.kind.title().to_string(),
        };
        let dir = installed.dir.clone();
        let repository = md.repository.clone();
        let mut panel = div()
            .flex()
            .flex_col()
            .w(zpx(DETAILS_WIDTH))
            .min_w_0()
            .overflow_hidden()
            .p(SPACING())
            .gap(zpx(2.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(md.display_name.clone()),
            )
            .children(md.description.clone().map(|d| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .whitespace_normal()
                    .child(d)
            }))
            .child(heading("Source"))
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .min_w_0()
                    .truncate()
                    .child(source),
            )
            .children(md.version.clone().map(|v| line(format!("Version {v}"))))
            .children(md.publisher.clone().map(|p| line(format!("By {p}"))))
            .children(md.license.clone().map(|l| line(format!("License {l}"))))
            .children(repository.map(|url| {
                let shown = url.trim_start_matches("https://").to_string();
                link_button("lang-ext-repository", shown, cx)
                    .text_size(FONT_SIZE_SM())
                    .min_w_0()
                    .truncate()
                    .on_click(move |_, _, cx| Dispatcher::open_url(&url, cx))
            }))
            .child(heading("Languages"));
        if md.languages.is_empty() {
            panel = panel.child(line("None declared.".to_string()).text_color(t.text_secondary));
        }
        if !md.languages.is_empty() {
            panel = panel.child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .whitespace_normal()
                    .child("Ticked languages win over the built-in highlighting for their files."),
            );
        }
        for language in &md.languages {
            let mut types: Vec<String> =
                language.suffixes.iter().map(|s| format!(".{s}")).collect();
            types.extend(language.filenames.iter().cloned());
            let name = language.name.clone().unwrap_or_else(|| language.id.clone());
            let preferred = extensions.language_preferred(&md.id, &language.id);
            let (ext_id, lang_id) = (md.id.clone(), language.id.clone());
            let slug = format!("{}-{}", install::slug(&md.id), install::slug(&language.id));
            panel = panel.child(
                div()
                    .id(SharedString::from(format!("lang-ext-lang-{slug}")))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(SPACING_HALF())
                    .cursor_pointer()
                    .child(div().pt(zpx(1.)).child(checkbox(
                        SharedString::from(format!("lang-ext-lang-box-{slug}")),
                        preferred,
                        false,
                        cx,
                    )))
                    .child(line(if types.is_empty() {
                        name
                    } else {
                        format!("{name}: {}", types.join(", "))
                    }))
                    .on_click(move |_, _, cx| {
                        Dispatcher::set_language_preferred(&ext_id, &lang_id, !preferred, cx)
                    }),
            );
        }
        // `117-file-icons`
        if !md.icon_themes.is_empty() {
            panel = panel.child(heading("File icon themes"));
            for theme in &md.icon_themes {
                panel = panel.child(line(theme.label.clone()));
            }
        }
        if !md.grammars.is_empty() || md.icon_themes.is_empty() {
            panel = panel.child(heading("Grammars"));
        }
        for grammar in &md.grammars {
            let kind = match grammar.kind {
                GrammarKind::TextMate => "TextMate",
                GrammarKind::Sublime => "Sublime",
                GrammarKind::TreeSitter => "tree-sitter",
            };
            let status = match (grammar.kind, grammar.status, &grammar.resolution) {
                (GrammarKind::TreeSitter, _, Resolution::Bundled { name }) => {
                    format!("uses Corvene's {name} grammar")
                }
                (GrammarKind::TreeSitter, _, Resolution::Built { .. }) => {
                    "built from source".to_string()
                }
                (GrammarKind::TreeSitter, _, Resolution::NeedsBuild) => {
                    "parser not available: needs a build from source".to_string()
                }
                (GrammarKind::TreeSitter, _, Resolution::Failed { error }) => error.clone(),
                (GrammarKind::TreeSitter, _, Resolution::Unresolved) => "not resolved".to_string(),
                (_, Status::Ok, _) if grammar.dropped > 0 => {
                    format!("ok, {} rule(s) dropped", grammar.dropped)
                }
                (_, Status::Ok, _) => "ok".to_string(),
                (_, Status::Slow, _) => "slow grammar".to_string(),
                (_, Status::Rejected, _) => grammar
                    .error
                    .clone()
                    .map(|e| format!("rejected: {e}"))
                    .unwrap_or_else(|| "rejected".to_string()),
            };
            let colour = match (grammar.status, &grammar.resolution) {
                (Status::Rejected, _) | (_, Resolution::Failed { .. }) => t.color_deleted,
                (Status::Slow, _) | (_, Resolution::NeedsBuild) => t.color_modified,
                _ => t.text,
            };
            panel = panel.child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .child(format!("{} ({kind})", grammar.name))
                    .child(
                        div()
                            .text_color(colour)
                            .text_size(FONT_SIZE_SM())
                            .min_w_0()
                            .whitespace_normal()
                            .child(status),
                    ),
            );
        }
        panel = panel.child(
            div().mt(SPACING()).child(
                link_button(
                    "lang-ext-reveal",
                    crate::context_menu::labels::REVEAL_IN_FILE_MANAGER,
                    cx,
                )
                .text_size(FONT_SIZE_SM())
                .on_click(move |_, _, cx| Dispatcher::show_in_finder(&dir, cx)),
            ),
        );
        div()
            .id("lang-ext-details-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(panel)
            .with_scrollbar()
            .into_any_element()
    }

    fn installed_tab(
        &self,
        extensions: &ExtensionsState,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let visible = self.visible(extensions, cx);
        let mut pending: Vec<(&String, &ExtensionProgress)> = extensions
            .progress
            .iter()
            .filter(|(k, _)| k.starts_with("pending:"))
            .collect();
        pending.sort_by(|a, b| a.0.cmp(b.0));
        let pending_errors: Vec<String> = extensions
            .errors
            .iter()
            .filter(|(k, _)| k.starts_with("pending:"))
            .map(|(k, e)| {
                let what = k
                    .strip_prefix("pending:")
                    .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                    .unwrap_or_default();
                format!("{what}: {e}")
            })
            .collect();
        let count = extensions.installed.len();
        let list_body: AnyElement = if visible.is_empty() && pending.is_empty() {
            div()
                .p(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(if count == 0 {
                    "Nothing installed yet. Add a file or folder below, or use Find and Import."
                } else {
                    "No extension matches the filter."
                })
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(pending.iter().map(|(k, p)| self.pending_row(k, p, cx)))
                .children(visible.iter().map(|i| self.row(i, extensions, cx)))
                .into_any_element()
        };
        let add_row = self.add_mode.map(|mode| {
            let label = match mode {
                AddMode::Url => "URL",
                AddMode::GitHub => "GitHub repository",
            };
            div()
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .px(SPACING())
                .py(SPACING_HALF())
                .border_t_1()
                .border_color(t.box_border)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(div().text_size(FONT_SIZE_SM()).flex_none().child(label))
                        .child(div().flex_1().child(text_box(
                            "lang-ext-add-input",
                            &self.add_input,
                            None,
                            window,
                            cx,
                        )))
                        .child(
                            small_button("lang-ext-add-submit", "Install", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.submit_add(cx))),
                        )
                        .child(
                            link_button("lang-ext-add-cancel", "Cancel", cx)
                                .text_size(FONT_SIZE_SM())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.add_mode = None;
                                    this.add_error = None;
                                    cx.notify();
                                })),
                        ),
                )
                .children(self.add_error.clone().map(|e| input_error(e, cx)))
        });
        let list = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.box_border)
            .child(
                div()
                    .p(SPACING())
                    .border_b_1()
                    .border_color(t.box_border)
                    .child(filter_text_box(
                        "lang-ext-filter",
                        &self.filter,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    )),
            )
            .child(
                div()
                    .id("lang-ext-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list_body)
                    .with_scrollbar(),
            )
            .children(pending_errors.into_iter().map(|e| {
                div()
                    .px(SPACING())
                    .py(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.color_deleted)
                    .whitespace_normal()
                    .border_t_1()
                    .border_color(t.box_border)
                    .child(e)
            }))
            .children(add_row);
        let details = div()
            .w(zpx(DETAILS_WIDTH))
            .flex_none()
            .bg(t.box_alt_background)
            .child(self.details(extensions, cx));
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(list)
            .child(details)
            .into_any_element()
    }

    // ---- Find ----

    fn candidate_row(
        &self,
        candidate: &Candidate,
        extensions: &ExtensionsState,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let id = registry::extension_id(candidate);
        let installed = extensions.get(&id).map(|i| i.metadata.version.clone());
        let installing = extensions
            .progress
            .contains_key(&InstallSource::Registry(candidate.clone()).key());
        let error = extensions
            .errors
            .get(&InstallSource::Registry(candidate.clone()).key())
            .cloned();
        let mut meta: Vec<String> = Vec::new();
        if let Some(publisher) = &candidate.publisher {
            meta.push(publisher.clone());
        }
        if let Some(version) = &candidate.version {
            meta.push(version.clone());
        }
        if candidate.downloads > 0 {
            meta.push(format!(
                "{} downloads",
                group_thousands(candidate.downloads)
            ));
        }
        let files = if candidate.icon_themes && candidate.suffixes.is_empty() {
            // `117-file-icons`
            "File icon theme".to_string()
        } else if candidate.suffixes.is_empty() {
            "File types: not listed by the registry (known once installed)".to_string()
        } else {
            let mut shown: Vec<String> = candidate
                .suffixes
                .iter()
                .map(|s| {
                    if s.contains('.') || s.chars().any(|c| c.is_ascii_uppercase()) {
                        s.clone()
                    } else {
                        format!(".{s}")
                    }
                })
                .collect();
            shown.sort();
            shown.dedup();
            format!("File types: {}", shown.join(" "))
        };
        let note = match candidate.grammar {
            GrammarHint::TreeSitter => Some(
                "tree-sitter grammar: works with a grammar Corvene bundles, else needs a build from source",
            ),
            GrammarHint::TextMate | GrammarHint::Unknown => None,
        };
        let action: AnyElement = match (installed, installing) {
            (_, true) => div()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child("Installing…")
                .into_any_element(),
            (Some(version), _) if version == candidate.version => div()
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child("Installed")
                .into_any_element(),
            (Some(_), _) => {
                let c = candidate.clone();
                small_button(
                    SharedString::from(format!("lang-ext-install-{id}")),
                    "Update",
                    cx,
                )
                .on_click(move |_, _, cx| {
                    Dispatcher::install_extension(InstallSource::Registry(c.clone()), cx)
                })
                .into_any_element()
            }
            (None, _) => {
                let c = candidate.clone();
                small_button(
                    SharedString::from(format!("lang-ext-install-{id}")),
                    "Install",
                    cx,
                )
                .on_click(move |_, _, cx| {
                    Dispatcher::install_extension(InstallSource::Registry(c.clone()), cx)
                })
                .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .px(SPACING())
            .py(SPACING_HALF())
            .gap(zpx(2.))
            .border_b_1()
            .border_color(t.box_border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .min_w_0()
                            .truncate()
                            .child(candidate.display_name.clone()),
                    )
                    .child(pill(
                        candidate.registry.title(),
                        None,
                        t.list_item_badge_background,
                        t.list_item_badge_text,
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(action),
            )
            .children(candidate.description.clone().map(|d| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .min_w_0()
                    .truncate()
                    .child(d)
            }))
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .min_w_0()
                    .truncate()
                    .child(meta.join("  ·  ")),
            )
            .child(
                div()
                    .text_size(FONT_SIZE_SM())
                    .min_w_0()
                    .whitespace_normal()
                    .child(files),
            )
            .children(note.map(|n| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.color_modified)
                    .child(n)
            }))
            .children(error.map(|e| {
                div()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.color_deleted)
                    .whitespace_normal()
                    .child(e)
            }))
            .into_any_element()
    }

    fn find_tab(
        &self,
        extensions: &ExtensionsState,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let search = &extensions.search;
        let mut status: Vec<String> = Vec::new();
        if !search.in_flight.is_empty() {
            status.push(format!(
                "Searching {}…",
                search
                    .in_flight
                    .iter()
                    .map(|r| r.title())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for registry in Registry::ALL {
            if let Some(err) = search.errors.get(&registry) {
                status.push(format!("{}: {err}", registry.title()));
            }
        }
        let body: AnyElement = if search.query.is_empty() && !search.icon_themes {
            div()
                .p(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .whitespace_normal()
                .child(
                    "Search Open VSX (VS Code extensions), Zed's and Pulsar's registries by name, \
                     or type a file suffix such as .nix to find extensions for that kind of file. \
                     Press Return to search.",
                )
                .into_any_element()
        } else if search.results.is_empty() && search.in_flight.is_empty() {
            div()
                .p(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(format!("Nothing found for {:?}.", search.query))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(
                    search
                        .results
                        .iter()
                        .map(|c| self.candidate_row(c, extensions, cx)),
                )
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .p(SPACING())
                    .border_b_1()
                    .border_color(t.box_border)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING())
                    .child(div().flex_1().child(text_box(
                        "lang-ext-query",
                        &self.query,
                        Some(octicon(Octicon::Search, t.text_secondary)),
                        window,
                        cx,
                    )))
                    .child(
                        button("lang-ext-search", "Search", cx).on_click(cx.listener(
                            |this, _, _, cx| {
                                let text = this.query.read(cx).value().to_string();
                                this.search(text, cx);
                            },
                        )),
                    )
                    // `117-file-icons`
                    .child({
                        let on = search.icon_themes;
                        div()
                            .id("lang-ext-icon-themes")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(SPACING_HALF())
                            .cursor_pointer()
                            .text_size(FONT_SIZE_SM())
                            .on_click(move |_, _, cx| {
                                Dispatcher::set_extension_search_icon_themes(!on, true, cx)
                            })
                            .child(crate::widgets::checkbox(
                                "lang-ext-icon-themes-check",
                                on,
                                false,
                                cx,
                            ))
                            .child("File icon themes")
                    }),
            )
            .children(status.into_iter().map(|s| {
                div()
                    .px(SPACING())
                    .py(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .whitespace_normal()
                    .child(s)
            }))
            .child(
                div()
                    .id("lang-ext-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body)
                    .with_scrollbar(),
            )
            .into_any_element()
    }

    // ---- Import ----

    fn import_tab(&self, extensions: &ExtensionsState, cx: &Context<Self>) -> AnyElement {
        let t = cx.ghd();
        let body: AnyElement = match &extensions.import_candidates {
            None => div()
                .p(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(if extensions.import_scanning {
                    "Looking for editors…"
                } else {
                    "Not scanned yet."
                })
                .into_any_element(),
            Some(candidates) if candidates.is_empty() => div()
                .p(SPACING())
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .whitespace_normal()
                .child(
                    "No grammar extensions found in VS Code, Cursor, VSCodium, Zed, Pulsar, Atom, \
                     Sublime Text or JetBrains folders on this Mac.",
                )
                .into_any_element(),
            Some(candidates) => {
                let mut sections = div().flex().flex_col();
                let mut current: Option<Editor> = None;
                for candidate in candidates {
                    if current != Some(candidate.editor) {
                        current = Some(candidate.editor);
                        sections = sections.child(
                            div()
                                .px(SPACING())
                                .pt(SPACING())
                                .pb(SPACING_HALF())
                                .text_size(FONT_SIZE_SM())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_secondary)
                                .child(candidate.editor.title()),
                        );
                    }
                    sections = sections.child(self.import_row(candidate, extensions, cx));
                }
                sections.into_any_element()
            }
        };
        div()
            .id("lang-ext-import")
            .size_full()
            .overflow_y_scroll()
            .child(body)
            .with_scrollbar()
            .into_any_element()
    }

    fn import_row(
        &self,
        candidate: &ImportCandidate,
        extensions: &ExtensionsState,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = cx.ghd();
        let id = install::extension_id(
            SourceKind::Imported,
            Some(&install::slug(candidate.editor.title())),
            &candidate.name,
        );
        let installed = extensions.get(&id).is_some();
        let installing = extensions
            .progress
            .contains_key(&InstallSource::Import(candidate.clone()).key());
        let checked = self.import_checked.contains(&candidate.path);
        let path = candidate.path.clone();
        let mut meta: Vec<String> = Vec::new();
        if let Some(v) = &candidate.version {
            meta.push(v.clone());
        }
        if !candidate.languages.is_empty() {
            meta.push(candidate.languages.join(", "));
        }
        if !candidate.suffixes.is_empty() {
            meta.push(
                candidate
                    .suffixes
                    .iter()
                    .map(|s| format!(".{s}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        if candidate.tree_sitter {
            meta.push("tree-sitter".to_string());
        }
        // `117-file-icons`
        if !candidate.icon_themes.is_empty() {
            meta.push(format!("File icons: {}", candidate.icon_themes.join(", ")));
        }
        let row_id = SharedString::from(format!(
            "lang-ext-import-row-{}",
            install::slug(&candidate.path.to_string_lossy())
        ));
        let check_id = SharedString::from(format!(
            "lang-ext-import-check-{}",
            install::slug(&candidate.path.to_string_lossy())
        ));
        let state_text = if installing {
            Some("Importing…")
        } else if installed {
            Some("Imported")
        } else {
            None
        };
        div()
            .id(row_id)
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .px(SPACING())
            .py(SPACING_HALF())
            .border_b_1()
            .border_color(t.box_border)
            .when(state_text.is_none(), |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.import_checked.remove(&path) {
                            this.import_checked.insert(path.clone());
                        }
                        cx.notify();
                    }))
            })
            .child(match state_text {
                Some(text) => div()
                    .w(zpx(64.))
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(text)
                    .into_any_element(),
                None => div()
                    .w(zpx(64.))
                    .flex_none()
                    .child(checkbox(check_id, checked, false, cx))
                    .into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .min_w_0()
                            .truncate()
                            .child(candidate.display_name.clone()),
                    )
                    .child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.text_secondary)
                            .min_w_0()
                            .truncate()
                            .child(meta.join("  ·  ")),
                    ),
            )
            .into_any_element()
    }

    fn import_selected(&mut self, cx: &mut Context<Self>) {
        let Some(candidates) = self.state.read(cx).extensions.import_candidates.clone() else {
            return;
        };
        for candidate in candidates {
            if self.import_checked.remove(&candidate.path) {
                Dispatcher::install_extension(InstallSource::Import(candidate), cx);
            }
        }
        cx.notify();
    }
}

fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The row's dot and state text.
fn extension_state(installed: &Installed, extensions: &ExtensionsState) -> (Hsla, String) {
    let md = &installed.metadata;
    let (dot, text) = if extensions.progress.contains_key(&md.id) {
        (hsla(0.12, 0.8, 0.5, 1.), "Updating…".to_string())
    } else if !md.enabled {
        (hsla(0., 0., 0.6, 1.), "Disabled".to_string())
    } else if md.grammars.is_empty() && !md.icon_themes.is_empty() {
        // `117-file-icons`: an icon theme alone has no grammar to use
        (hsla(0.33, 0.6, 0.45, 1.), "Enabled".to_string())
    } else if !md.usable() {
        (hsla(0., 0.7, 0.5, 1.), "No usable grammar".to_string())
    } else if md.grammars.iter().any(|g| {
        g.status == Status::Rejected
            || matches!(
                g.resolution,
                Resolution::NeedsBuild | Resolution::Failed { .. }
            )
    }) {
        (hsla(0.12, 0.8, 0.5, 1.), "Partly working".to_string())
    } else if extensions.rebuilding {
        (hsla(0.33, 0.6, 0.45, 1.), "Loading…".to_string())
    } else {
        (hsla(0.33, 0.6, 0.45, 1.), "Enabled".to_string())
    };
    (dot, text)
}

fn grammar_kind_label(installed: &Installed) -> &'static str {
    let kinds: Vec<GrammarKind> = installed.metadata.grammars.iter().map(|g| g.kind).collect();
    let tree_sitter = kinds.contains(&GrammarKind::TreeSitter);
    let textmate = kinds
        .iter()
        .any(|k| matches!(k, GrammarKind::TextMate | GrammarKind::Sublime));
    match (textmate, tree_sitter) {
        (true, true) => "TextMate + tree-sitter",
        (false, true) => "tree-sitter",
        _ => "TextMate",
    }
}

impl Render for LanguageExtensionsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd();
        let extensions = self.state.read(cx).extensions.clone();
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_language_extensions(cx);
        let count = extensions.installed.len();
        let tabs = vec![
            TabModel {
                id: "lang-ext-tab-installed",
                label: "Installed".into(),
                count: (count > 0).then(|| count.to_string().into()),
                dot: false,
            },
            TabModel {
                id: "lang-ext-tab-find",
                label: "Find".into(),
                count: None,
                dot: false,
            },
            TabModel {
                id: "lang-ext-tab-import",
                label: "Import from Editors".into(),
                count: None,
                dot: false,
            },
        ];
        let selected = match self.tab {
            Tab::Installed => 0,
            Tab::Find => 1,
            Tab::Import => 2,
        };
        let this = cx.entity().clone();
        let bar = tab_bar(
            tabs,
            selected,
            move |ix, _, cx| {
                this.update(cx, |d, cx| {
                    d.set_tab(
                        match ix {
                            0 => Tab::Installed,
                            1 => Tab::Find,
                            _ => Tab::Import,
                        },
                        cx,
                    )
                })
            },
            cx,
        );
        let body = match self.tab {
            Tab::Installed => self.installed_tab(&extensions, window, cx),
            Tab::Find => self.find_tab(&extensions, window, cx),
            Tab::Import => self.import_tab(&extensions, cx),
        };
        let consent = extensions
            .pending_consent
            .as_ref()
            .map(|c| self.consent_sheet(c, cx));
        let content = crate::dialog::own_enter(div())
            .w(crate::theme::fit_width(WIDTH))
            .max_w_full()
            .min_w_0()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .border_b_1()
                    .border_color(t.box_border)
                    .child(bar),
            )
            .child(
                div()
                    .h(zpx(HEIGHT))
                    .w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .child(body),
            )
            .children(consent);
        let summary = match count {
            0 => "No extensions installed.".to_string(),
            1 => "1 extension installed.".to_string(),
            n => format!("{n} extensions installed."),
        };
        let mut footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING())
            .px(SPACING_DOUBLE())
            .py(SPACING())
            .border_t_1()
            .border_color(t.box_border);
        footer = match self.tab {
            Tab::Installed => footer
                .child(
                    button("lang-ext-add", "Add File or Folder…", cx)
                        .on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
                )
                .child(
                    button("lang-ext-add-url", "From URL…", cx)
                        .on_click(cx.listener(|this, _, window, cx| this.open_add(AddMode::Url, window, cx))),
                )
                .child(
                    button("lang-ext-add-github", "From GitHub…", cx)
                        .on_click(cx.listener(|this, _, window, cx| this.open_add(AddMode::GitHub, window, cx))),
                )
                .child(settings_description(cx).mt(zpx(0.)).child(summary)),
            Tab::Find => footer.child(settings_description(cx).mt(zpx(0.)).child(
                "Only open registries are searched; Corvene never contacts the Visual Studio Marketplace.",
            )),
            Tab::Import => {
                let checked = self.import_checked.len();
                footer
                    .child(
                        button("lang-ext-rescan", "Rescan", cx)
                            .on_click(|_, _, cx| Dispatcher::scan_installed_editors(cx)),
                    )
                    .child(
                        button(
                            "lang-ext-import-selected",
                            if checked == 0 {
                                "Import Selected".to_string()
                            } else {
                                format!("Import Selected ({checked})")
                            },
                            cx,
                        )
                        .when(checked == 0, |d| d.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| this.import_selected(cx))),
                    )
                    .child(settings_description(cx).mt(zpx(0.)).child(
                        "Copies the extension into Corvene; the editor's copy is untouched.",
                    ))
            }
        };
        footer = footer.child(div().flex_1()).child(
            primary_button("lang-ext-close", "Close", false, cx)
                .on_click(|_, _, cx| Dispatcher::close_language_extensions(cx)),
        );
        dialog_with_frame(
            "language-extensions",
            "Language Extensions",
            content,
            Vec::new(),
            DialogFrame {
                content_padding: false,
                footer: Some(footer.into_any_element()),
                ..Default::default()
            },
            close,
            window,
            cx,
        )
    }
}
