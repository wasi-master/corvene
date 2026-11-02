//! Branch dialogs: `ui/create-branch/create-branch-dialog.tsx`,
//! `ui/rename-branch/rename-branch-dialog.tsx`, `ui/delete-branch/delete-branch-dialog.tsx`,
//! `ui/stash-changes/{stash-and-switch-branch,overwrite-stashed-changes}-dialog.tsx`
//! and the merge `ChooseBranch` step (`merge-choose-branch-dialog.tsx`), with
//! the helpers they share: `lib/sanitize-ref-name.ts` and
//! `ui/lib/ref-name-text-box.tsx` (`sanitize_ref_name`, `ref_name_notice`),
//! `lib/create-branch.ts` (`get_start_point`) and
//! `ui/lib/branch-name-warnings.tsx`.
//!
//! Deviations: Create a Branch can start from any branch through an "Other
//! branch…" choice (`843-create-branch-from-any-branch`) and preselects the
//! current branch while there are uncommitted changes
//! (`844-create-branch-with-changes-from-current`).
//! Delete Branch warns about unmerged commits and a stash on the branch
//! (`860-delete-branch-warnings`), names the upstream in its "delete on the
//! remote" checkbox and hides it for the remote's default branch
//! (`870-delete-remote-names-upstream`).
//! Create and Rename refuse `head` in any case (`846-reject-head-branch-name`).
//! Create a Branch also names the branch of "Commit to New Branch…" and
//! "Create Branch from Commits…" (`787-commit-to-new-branch`).
//! Rename Branch focuses the name box, not the close button
//! (`872-rename-branch-focuses-name`).
//! Create a Branch can prefill a name prefix (`845-branch-name-prefix`).
//! Branch names can have more characters replaced with `-`
//! (`873-branch-name-forbidden-chars`).
//! `ConfirmSwitchBranchDialog` is a Corvene addition (`864-confirm-branch-switch`).
//! `DropKeptStashDialog` is a Corvene addition (`774-stash-conflict-flow`).
//! Overwrite Stash can add the changes to the stash instead
//! (`776-stash-add-to-existing`).
//! Switch Branch can discard the changes instead (`865-switch-branch-discard`)
//! and warns when the branch the changes go to is behind its upstream
//! (`1207-switch-warns-target-behind`).
//! Squash and merge has commit message fields (flag `837`).
//! New Branch and Rename Branch check the name against the repository rules
//! (GHD `ui/lib/branch-name-rule-validation.tsx`); "repo rules" in their
//! message links to the branch's rulesets (`1225-branch-rules-link`, GHD
//! 3.6.6 shows plain text).

use corvene_core::{
    AppState, Branch, BranchKind, Dispatcher, Mergeability, Tip, UncommittedChangesStrategy,
};
use std::time::{Duration, UNIX_EPOCH};

use gpui_kit::component::input::{InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::branch_list::group_branches;
use crate::context_menu::mac_or;
use crate::dialog::{
    DialogButton, DialogFrame, DialogKind, dialog, dialog_framed, dialog_with_kind,
};
use crate::icons::{Octicon, octicon};
use crate::relative_time::relative;
use crate::scrollbar::ScrollbarExt;
use crate::theme::ActiveGhdTheme;
use crate::theme::sizes::*;
use crate::widgets::{Inline, checkbox, paragraph, segmented_option, text_box};

/// GHD `sanitizedRefName` (`lib/sanitize-ref-name.ts`), what its
/// `RefNameTextBox` turns the input into: each run of control characters,
/// spaces and `~ ^ : ? * [ \ | " < >`, each `@{`, each run of two or more
/// dots, a leading or trailing dot, a trailing `.lock` and a trailing `/`
/// becomes `-` (`git check-ref-format`, plus what Windows refuses); then
/// leading `-` and `+` go.
pub fn sanitize_ref_name(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let invalid = |c: char| {
        c <= '\u{20}'
            || c == '\u{7f}'
            || matches!(
                c,
                '~' | '^' | ':' | '?' | '*' | '[' | '\\' | '|' | '"' | '<' | '>'
            )
    };
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let rest = &chars[i..];
        let at_end = rest.len() == 1;
        let run = if invalid(rest[0]) {
            rest.iter().take_while(|c| invalid(**c)).count()
        } else if rest.starts_with(&['@', '{']) {
            2
        } else if rest.starts_with(&['.', '.']) {
            rest.iter().take_while(|c| **c == '.').count()
        } else if rest[0] == '.' && (i == 0 || at_end) {
            1
        } else if rest == ['.', 'l', 'o', 'c', 'k'] {
            5
        } else if rest[0] == '/' && at_end {
            1
        } else {
            0
        };
        if run == 0 {
            out.push(rest[0]);
            i += 1;
        } else {
            out.push('-');
            i += run;
        }
    }
    out.trim_start_matches(['-', '+']).to_string()
}

/// GHD `RefNameTextBox.renderRefValueWarningError`: under a ref name box
/// whose input [`sanitize_ref_name`] changed, "Will be `verb` as <name>."
/// (`InputWarning`), or "<input> is not a valid name." (`InputError`) when
/// nothing is left.
pub(crate) fn ref_name_notice(proposed: &str, verb: &'static str, cx: &App) -> Option<AnyElement> {
    ref_name_notice_with(proposed, "", verb, cx)
}

/// [`ref_name_notice`] for a box that also replaces each character of
/// `forbidden` ([`sanitize_ref_name_with`], `873-branch-name-forbidden-chars`).
/// With `871-branch-name-trailing-slash-quiet` nothing shows while the only
/// difference is a trailing `/` or `.` ([`ref_name_warning`]).
pub(crate) fn ref_name_notice_with(
    proposed: &str,
    forbidden: &str,
    verb: &'static str,
    cx: &App,
) -> Option<AnyElement> {
    let sanitized = sanitize_ref_name_with(proposed, forbidden);
    let quiet = AppState::global(cx)
        .read(cx)
        .flags
        .bool(corvene_core::flags::ids::BRANCH_NAME_TRAILING_SLASH_QUIET);
    if sanitized == proposed || (quiet && only_trailing_separators(proposed, forbidden)) {
        return None;
    }
    let t = cx.ghd();
    let (icon, icon_color, text_color, parts): (_, _, _, Vec<Inline>) = if sanitized.is_empty() {
        (
            Octicon::Stop,
            t.input_error_text,
            t.input_error_text,
            vec![
                ref_chip(proposed.to_string(), cx).into_any_element().into(),
                " is not a valid name.".into(),
            ],
        )
    } else {
        (
            Octicon::Alert,
            t.dialog_warning,
            t.text_secondary,
            vec![
                format!("Will be {verb} as ").into(),
                ref_chip(sanitized, cx).into_any_element().into(),
                ".".into(),
            ],
        )
    };
    // `.input-description`: small text, the octicon half a spacing before
    Some(
        div()
            .flex()
            .flex_row()
            .items_start()
            .text_size(FONT_SIZE_SM())
            .line_height(zpx(16.5))
            .text_color(text_color)
            .child(octicon(icon, icon_color).flex_none().mr(SPACING_HALF()))
            .child(paragraph(parts).flex_1().min_w_0())
            .into_any_element(),
    )
}

/// [`sanitize_ref_name`] after replacing each character of `forbidden`
/// with `-` (`873-branch-name-forbidden-chars`).
pub fn sanitize_ref_name_with(input: &str, forbidden: &str) -> String {
    let replaced: String = input
        .chars()
        .map(|c| {
            if !c.is_whitespace() && forbidden.contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    sanitize_ref_name(&replaced)
}

/// What a new branch name box turns its input into: [`sanitize_ref_name`]
/// plus the characters the `873-branch-name-forbidden-chars` flag lists.
pub fn sanitize_branch_name(input: &str, cx: &App) -> String {
    sanitize_ref_name_with(input, &branch_forbidden_chars(cx))
}

/// The characters `873-branch-name-forbidden-chars` adds to a branch name
/// box's replacements (none when it is off).
pub fn branch_forbidden_chars(cx: &App) -> String {
    AppState::global(cx)
        .read(cx)
        .flags
        .text(corvene_core::flags::ids::BRANCH_NAME_FORBIDDEN_CHARS)
        .to_string()
}

/// Whether a ref name box shows "Will be … as <sanitized>" for `raw`
/// (sanitized with [`sanitize_ref_name_with`] and `forbidden`).
/// `quiet_trailing` (`871-branch-name-trailing-slash-quiet`) keeps it hidden
/// while the only difference is a trailing `/` or `.`, typed on the way to
/// `feature/x` (GHD flashes "Will be created as feature-").
pub fn ref_name_warning(raw: &str, forbidden: &str, quiet_trailing: bool) -> bool {
    let raw = raw.trim();
    !raw.is_empty()
        && sanitize_ref_name_with(raw, forbidden) != raw
        && !(quiet_trailing && only_trailing_separators(raw, forbidden))
}

/// `871-branch-name-trailing-slash-quiet`: `raw` is a valid name followed by
/// one or more `/` or `.`.
fn only_trailing_separators(raw: &str, forbidden: &str) -> bool {
    let head = raw.trim_end_matches(['/', '.']);
    head.len() < raw.len() && !head.is_empty() && sanitize_ref_name_with(head, forbidden) == head
}

/// Flag `846-reject-head-branch-name`: `head` in any case names `HEAD`
/// on a case-insensitive file system, so the new branch detaches HEAD.
fn reserved_head_name(name: &str, cx: &App) -> bool {
    name.eq_ignore_ascii_case("head")
        && AppState::global(cx)
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::REJECT_HEAD_BRANCH_NAME)
}

/// The message shown for a name `reserved_head_name` rejects.
fn reserved_head_message(name: &str) -> String {
    format!("{name} is reserved by Git (HEAD); choose another name.")
}

pub(crate) fn ref_chip(name: impl Into<SharedString>, cx: &App) -> Div {
    crate::widgets::code_ref(name, cx)
}

/// A warning of GHD `ui/lib/branch-name-warnings.tsx`: its text around a
/// `<Ref>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchNameWarning {
    pub before: &'static str,
    pub reference: String,
    pub after: &'static str,
}

impl BranchNameWarning {
    /// The warning's text.
    pub fn text(&self) -> String {
        format!("{}{}{}", self.before, self.reference, self.after)
    }
}

/// GHD `renderBranchHasRemoteWarning(branch)` (Rename Branch): renaming a
/// branch that tracks an upstream leaves the remote branch's name alone.
pub fn render_branch_has_remote_warning(branch: &Branch) -> Option<BranchNameWarning> {
    branch.upstream_short().map(|upstream| BranchNameWarning {
        before: "This branch is tracking ",
        reference: upstream.to_string(),
        after: " and renaming this branch will not change the branch name on the remote.",
    })
}

/// GHD `renderBranchNameExistsOnRemoteWarning(sanitizedName, branches)`
/// (Create Branch): a remote branch of `branches` has the name.
pub fn render_branch_name_exists_on_remote_warning(
    sanitized_name: &str,
    branches: &[Branch],
) -> Option<BranchNameWarning> {
    branches
        .iter()
        .any(|b| b.kind == BranchKind::Remote && b.name_without_remote() == sanitized_name)
        .then(|| BranchNameWarning {
            before: "A branch named ",
            reference: sanitized_name.to_string(),
            after: " already exists on the remote.",
        })
}

/// `<Row className="warning-helper-text">`: the alert octicon and the
/// warning, its reference as a `<Ref>`.
fn branch_name_warning(warning: BranchNameWarning, cx: &App) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(SPACING_HALF())
        .child(octicon(Octicon::Alert, cx.ghd().dialog_warning))
        .child(
            paragraph(vec![
                warning.before.into(),
                ref_chip(warning.reference, cx).into_any_element().into(),
                warning.after.into(),
            ])
            .flex_1()
            .min_w_0(),
        )
}

// ---------------------------------------------------------------------------

/// GHD `StartPoint` (`models/branch.ts`): what Create a Branch bases the
/// new branch on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartPoint {
    CurrentBranch,
    DefaultBranch,
    Head,
    UpstreamDefaultBranch,
}

/// GHD `getStartPoint(props, preferred)` (`lib/create-branch.ts`): a
/// detached HEAD always gives `Head`; otherwise `preferred` when it is
/// available, else the upstream default branch, the default branch, the
/// current branch or `Head`, the first that is.
pub fn get_start_point(
    tip: &Tip,
    default_branch: Option<&Branch>,
    upstream_default_branch: Option<&Branch>,
    preferred: StartPoint,
) -> StartPoint {
    if matches!(tip, Tip::Detached { .. }) {
        return StartPoint::Head;
    }
    let valid = matches!(tip, Tip::Valid { .. });
    let available = |point| match point {
        StartPoint::UpstreamDefaultBranch => upstream_default_branch.is_some(),
        StartPoint::DefaultBranch => default_branch.is_some(),
        StartPoint::CurrentBranch => valid,
        StartPoint::Head => true,
    };
    if available(preferred) {
        return preferred;
    }
    [
        StartPoint::UpstreamDefaultBranch,
        StartPoint::DefaultBranch,
        StartPoint::CurrentBranch,
    ]
    .into_iter()
    .find(|point| available(*point))
    .unwrap_or(StartPoint::Head)
}

/// The dialog's "Create branch based on…" choice.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Base {
    /// GHD's preferred start point, resolved with [`get_start_point`]
    /// against the current tip and branches on every render (GHD
    /// `componentWillReceiveProps`).
    Start(StartPoint),
    /// `843-create-branch-from-any-branch`: a branch picked from a list.
    Other,
}

/// GHD `checkBranchRules` delay after the last keystroke.
const BRANCH_RULES_DEBOUNCE: Duration = Duration::from_millis(500);

/// GHD `branchRulesDebounceId`: asks for the repository rules of the name
/// once it has rested [`BRANCH_RULES_DEBOUNCE`].
#[derive(Default)]
struct BranchRulesCheck {
    name: String,
    _task: Option<Task<()>>,
}

impl BranchRulesCheck {
    fn schedule<T: 'static>(&mut self, repo: u64, name: String, cx: &mut Context<T>) {
        if name == self.name {
            return;
        }
        self.name = name.clone();
        self._task = (!name.is_empty()).then(|| {
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(BRANCH_RULES_DEBOUNCE).await;
                cx.update(|cx| Dispatcher::check_branch_name_rules(repo, name, cx));
            })
        });
    }
}

/// GHD `renderBranchNameRuleError`: "Branch name '<name>' is restricted by
/// repo rules." as an `InputError`, or with ", but you can bypass them.
/// Proceed with caution!" as an `InputWarning`, while the result is for
/// `name`. The flag tells whether it is an error (OK disabled).
///
/// Deviation (`1225-branch-rules-link`; GHD 3.6.6 shows the text only,
/// desktop#21824): "repo rules" links to the rulesets that apply to the
/// branch, like the commit form's "one or more rules".
fn branch_name_rule_notice(
    state: &AppState,
    repo: u64,
    name: &str,
    cx: &App,
) -> Option<(AnyElement, bool)> {
    let error = state
        .repo_states
        .get(&repo)?
        .branch_name_rule_error
        .as_ref()
        .filter(|e| e.name == name)?;
    let t = cx.ghd();
    let link = state
        .flags
        .bool(corvene_core::flags::ids::BRANCH_RULES_LINK)
        .then(|| {
            crate::changes::repo_rulesets_for_branch_link(
                state.repository(repo).and_then(|r| r.github.as_ref()),
                Some(name),
            )
        })
        .flatten();
    let rules: Inline = match link {
        Some(url) => crate::widgets::link_button("branch-name-rules-link", "repo rules", cx)
            .on_click(move |_, _, cx| Dispatcher::open_url(&url, cx))
            .into_any_element()
            .into(),
        None => "repo rules".into(),
    };
    let tail = if error.is_warning {
        ", but you can bypass them. Proceed with caution!"
    } else {
        "."
    };
    let parts = vec![
        format!("Branch name '{name}' is restricted by ").into(),
        rules,
        tail.into(),
    ];
    // `.input-description-warning` / `-error`
    let (icon, icon_color, text_color) = if error.is_warning {
        (Octicon::Alert, t.dialog_warning, t.text_secondary)
    } else {
        (Octicon::Stop, t.input_error_text, t.input_error_text)
    };
    let row = div()
        .flex()
        .flex_row()
        .items_start()
        .text_size(FONT_SIZE_SM())
        .line_height(zpx(16.5))
        .text_color(text_color)
        .child(octicon(icon, icon_color).flex_none().mr(SPACING_HALF()))
        .child(paragraph(parts).flex_1().min_w_0())
        .into_any_element();
    Some((row, !error.is_warning))
}

pub struct CreateBranchDialog {
    state: Entity<AppState>,
    repo: u64,
    target_sha: Option<String>,
    name: Entity<InputState>,
    start_point: Base,
    /// `843-create-branch-from-any-branch`: the "Other branch…" picker.
    other_filter: Entity<InputState>,
    other_focus: FocusHandle,
    other_branch: Option<String>,
    /// Cherry-pick › New Branch: "Cherry-pick to New Branch" / "Create Branch and Cherry-pick".
    cherry_pick: bool,
    /// `787-commit-to-new-branch`: what the new branch is for.
    purpose: Purpose,
    /// `787-commit-to-new-branch`: "Remove them from <branch>" (unticked
    /// until the user asks).
    remove_from_current: bool,
    rules_check: BranchRulesCheck,
}

/// `787-commit-to-new-branch`: what OK does with the branch name.
type NameThen = Box<dyn Fn(String, &mut App)>;

/// `787-commit-to-new-branch`: the Create a Branch dialog's other uses.
enum Purpose {
    Branch,
    /// The commit form's "Commit to New Branch…".
    Commit {
        summary: String,
        description: String,
    },
    /// History's "Create Branch from Commits…".
    FromCommits(corvene_core::new_branch_flows::FromCommitsPlan),
}

impl CreateBranchDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        target_sha: Option<String>,
        initial_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        // `845-branch-name-prefix`
        let prefix = state
            .read(cx)
            .flags
            .text(corvene_core::flags::ids::BRANCH_NAME_PREFIX)
            .to_string();
        let initial_name = if initial_name.starts_with(&prefix) {
            initial_name
        } else {
            format!("{prefix}{initial_name}")
        };
        if !initial_name.is_empty() {
            name.update(cx, |s, cx| s.set_value(initial_name, window, cx));
        }
        cx.observe(&name, |this: &mut Self, _, cx| {
            this.check_rules(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // `RefNameTextBox` autoFocus
        let handle = name.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        let other_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&other_filter, |_, _, cx| cx.notify()).detach();
        // `844-create-branch-with-changes-from-current`: uncommitted changes
        // come along, so start where they were written
        let start_point = {
            let s = state.read(cx);
            let has_changes = s
                .repo_states
                .get(&repo)
                .and_then(|r| r.status.as_deref())
                .is_some_and(|st| !st.files.is_empty());
            if has_changes
                && s.flags
                    .bool(corvene_core::flags::ids::CREATE_BRANCH_WITH_CHANGES_FROM_CURRENT)
            {
                Base::Start(StartPoint::CurrentBranch)
            } else {
                // GHD's constructor: `getStartPoint(props, UpstreamDefaultBranch)`
                Base::Start(StartPoint::UpstreamDefaultBranch)
            }
        };
        Self {
            state,
            repo,
            target_sha,
            name,
            start_point,
            other_filter,
            other_focus: cx.focus_handle(),
            other_branch: None,
            cherry_pick: false,
            purpose: Purpose::Branch,
            remove_from_current: false,
            rules_check: BranchRulesCheck::default(),
        }
    }

    /// GHD `updateBranchName`: check the repository rules once the name
    /// rests.
    fn check_rules(&mut self, cx: &mut Context<Self>) {
        let name = sanitize_branch_name(self.name.read(cx).value().as_ref(), cx);
        self.rules_check.schedule(self.repo, name, cx);
    }

    /// `787-commit-to-new-branch`: "Commit to New Branch…": the branch
    /// starts at `HEAD` with the changes and the commit lands there.
    pub fn new_for_commit(
        state: Entity<AppState>,
        repo: u64,
        summary: String,
        description: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(state, repo, None, String::new(), window, cx);
        this.purpose = Purpose::Commit {
            summary,
            description,
        };
        this
    }

    /// `787-commit-to-new-branch`: History's "Create Branch from Commits…".
    pub fn new_from_commits(
        state: Entity<AppState>,
        repo: u64,
        plan: corvene_core::new_branch_flows::FromCommitsPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(state, repo, None, String::new(), window, cx);
        this.purpose = Purpose::FromCommits(plan);
        this
    }

    /// `787-commit-to-new-branch`: what the dialog says instead of the start
    /// point choice (the branch always starts at `HEAD`).
    fn purpose_description(&self, cx: &mut Context<Self>) -> Option<Vec<AnyElement>> {
        let github = self
            .state
            .read(cx)
            .repository(self.repo)
            .is_some_and(|r| r.github.is_some());
        let then = if github {
            " Then the branch is published and its pull request page opens."
        } else {
            " Then the branch is published."
        };
        match &self.purpose {
            Purpose::Branch => None,
            Purpose::Commit { .. } => Some(vec![
                div()
                    .child(format!(
                        "Your changes are committed on the new branch, which starts at your \
                         current commit.{then}"
                    ))
                    .into_any_element(),
            ]),
            Purpose::FromCommits(plan) => {
                let n = plan.commits.len();
                let mut parts = vec![
                    div()
                        .child(format!(
                            "The new branch starts at the newest of the {n} selected commits, so \
                             it has them all.{then}"
                        ))
                        .into_any_element(),
                ];
                if let Some(back) = &plan.move_back {
                    let ticked = self.remove_from_current;
                    let weak = cx.weak_entity();
                    parts.push(
                        crate::widgets::checkbox_row(
                            "create-branch-remove-commits",
                            ticked,
                            format!("Remove the {n} commits from {}", back.branch),
                            move |value, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.remove_from_current = value;
                                    cx.notify();
                                })
                                .ok();
                            },
                            cx,
                        )
                        .into_any_element(),
                    );
                    if ticked {
                        let short = &back.target[..back.target.len().min(7)];
                        parts.push(
                            div()
                                .text_color(cx.ghd().text_secondary)
                                .child(format!(
                                    "{} goes back to commit {short}. The commits stay on the new \
                                     branch; none of them has been pushed.",
                                    back.branch
                                ))
                                .into_any_element(),
                        );
                    }
                }
                Some(parts)
            }
        }
    }

    /// The `CreateBranch` step of a cherry-pick (`renderCreateBranch`).
    pub fn new_for_cherry_pick(
        state: Entity<AppState>,
        repo: u64,
        initial_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(state, repo, None, initial_name, window, cx);
        this.cherry_pick = true;
        this
    }
}

impl Render for CreateBranchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cherry_pick = self.cherry_pick;
        let repo_for_close = self.repo;
        let close = move |_: &mut Window, cx: &mut App| {
            if cherry_pick {
                Dispatcher::end_mco(repo_for_close, cx);
            } else {
                Dispatcher::close_popup(cx);
            }
        };
        let raw = self.name.read(cx).value().to_string();
        let name = sanitize_branch_name(&raw, cx);
        let (tip, default_branch, default_branch_ref, existing, target_commit, remote_warning) = {
            let s = self.state.read(cx);
            let rs = s.repo_states.get(&self.repo);
            let info = rs.and_then(|r| r.info.as_ref());
            // GHD's `allBranches`: tracked remote branches merged into their
            // local branch
            let all_branches = info
                .map(|i| crate::branch_list::merge_remote_and_local_branches(&i.branches))
                .unwrap_or_default();
            let remote_warning = render_branch_name_exists_on_remote_warning(&name, &all_branches);
            // `updateBranchName`: any of `allBranches`, remote names included
            let existing: Vec<String> = all_branches.into_iter().map(|b| b.name).collect();
            let target = self.target_sha.as_ref().and_then(|sha| {
                rs.and_then(|r| r.find_commit(sha))
                    .map(|c| (c.summary.clone(), c.short_sha().to_string()))
            });
            let default_branch = rs.and_then(|r| r.default_branch.clone());
            // GHD `findDefaultBranch`: the local branch, else the remote one
            let default_branch_ref = default_branch.as_deref().and_then(|d| {
                info.and_then(|i| {
                    i.branches
                        .iter()
                        .find(|b| b.kind == BranchKind::Local && b.name == d)
                        .or_else(|| {
                            i.branches.iter().find(|b| {
                                b.kind == BranchKind::Remote && b.name_without_remote() == d
                            })
                        })
                        .cloned()
                })
            });
            (
                info.map(|i| i.tip.clone()).unwrap_or(Tip::Unknown),
                default_branch,
                default_branch_ref,
                existing,
                target,
                remote_warning,
            )
        };
        let exists = existing.contains(&name);
        let reserved = reserved_head_name(&name, cx);
        let current = tip.branch_name().map(|s| s.to_string());
        let from_any = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::CREATE_BRANCH_FROM_ANY_BRANCH);
        // "Other branch…" chosen but no branch picked yet
        let mut needs_pick = false;

        // Where the branch starts from (`renderBranchDescription`).
        let mut description: Vec<AnyElement> = Vec::new();
        let mut start_point: Option<String> = None;
        if let Some(purpose) = self.purpose_description(cx) {
            description.extend(purpose);
        } else if let Some((summary, short)) = &target_commit {
            description.push(
                div()
                    .child(format!(
                        "Your new branch will be based on the commit '{summary}' ({short}) from your repository."
                    ))
                    .into_any_element(),
            );
            start_point = self.target_sha.clone();
        } else {
            match &tip {
                Tip::Detached { sha } => description.push(
                    div()
                        .child(format!(
                            "You do not currently have any branch checked out (your HEAD reference is detached). As such your new branch will be based on your currently checked out commit ({}).",
                            &sha[..sha.len().min(7)]
                        ))
                        .into_any_element(),
                ),
                Tip::Unborn { .. } => description.push(
                    div()
                        .child("Your current branch is unborn (does not contain any commits). Creating a new branch will rename the current branch.")
                        .into_any_element(),
                ),
                Tip::Valid { branch } => {
                    let current_name = branch.name.clone();
                    let other_default =
                        default_branch.clone().filter(|d| *d != current_name);
                    // GHD `renderForkBranchSelection`
                    let upstream_default = self
                        .state
                        .read(cx)
                        .upstream_default_branch_of(self.repo)
                        .cloned();
                    let upstream_is_current = upstream_default
                        .as_ref()
                        .is_some_and(|u| u.name_without_remote() == current_name);
                    let parent_name = self
                        .state
                        .read(cx)
                        .repository(self.repo)
                        .and_then(|r| r.github.as_ref())
                        .and_then(|g| g.parent.as_ref())
                        .map(|p| format!("{}/{}", p.owner, p.name));
                    if let (Some(upstream), true, false) =
                        (&upstream_default, upstream_is_current, from_any)
                    {
                        start_point = Some(upstream.name.clone());
                        description.push(
                            div()
                                .child(format!(
                                    "Your new branch will be based on {}'s default branch ({}).",
                                    parent_name.clone().unwrap_or_default(),
                                    upstream.name_without_remote()
                                ))
                                .into_any_element(),
                        );
                    } else if other_default.is_some() || upstream_default.is_some() || from_any {
                        // `selectedValue`: the default branch when that is
                        // the start point and not the current branch, else
                        // the current branch
                        let selected = match self.start_point {
                            Base::Other => Base::Other,
                            Base::Start(preferred) => match get_start_point(
                                &tip,
                                default_branch_ref.as_ref(),
                                upstream_default.as_ref(),
                                preferred,
                            ) {
                                StartPoint::UpstreamDefaultBranch => {
                                    Base::Start(StartPoint::UpstreamDefaultBranch)
                                }
                                StartPoint::DefaultBranch if other_default.is_some() => {
                                    Base::Start(StartPoint::DefaultBranch)
                                }
                                _ => Base::Start(StartPoint::CurrentBranch),
                            },
                        };
                        start_point = match selected {
                            Base::Start(StartPoint::UpstreamDefaultBranch) => {
                                upstream_default.as_ref().map(|u| u.name.clone())
                            }
                            Base::Start(StartPoint::DefaultBranch) => other_default.clone(),
                            Base::Start(_) => Some(current_name.clone()),
                            Base::Other => {
                                needs_pick = self.other_branch.is_none();
                                self.other_branch.clone()
                            }
                        };
                        let mut options: Vec<(&'static str, String, &'static str, Base)> =
                            Vec::new();
                        if let Some(upstream) = &upstream_default {
                            options.push((
                                "start-upstream",
                                upstream.name.clone(),
                                "The default branch of the upstream repository. Pick this to start on something new that's not dependent on your current branch.",
                                Base::Start(StartPoint::UpstreamDefaultBranch),
                            ));
                        } else if let Some(default) = &other_default {
                            options.push((
                                "start-default",
                                default.clone(),
                                "The default branch in your repository. Pick this to start on something new that's not dependent on your current branch.",
                                Base::Start(StartPoint::DefaultBranch),
                            ));
                        }
                        options.push((
                            "start-current",
                            current_name.clone(),
                            "The currently checked out branch. Pick this if you need to build on work done on this branch.",
                            Base::Start(StartPoint::CurrentBranch),
                        ));
                        if from_any {
                            options.push((
                                "start-other",
                                match (&self.other_branch, selected) {
                                    (Some(other), Base::Other) => {
                                        format!("Other branch: {other}")
                                    }
                                    _ => "Other branch…".to_string(),
                                },
                                "Any local or remote branch, picked from the list below.",
                                Base::Other,
                            ));
                        }
                        let last = options.len() - 1;
                        let picker = (selected == Base::Other).then(|| {
                            let groups = {
                                let s = self.state.read(cx);
                                let query = self.other_filter.read(cx).value().trim().to_string();
                                match s.repo_states.get(&self.repo) {
                                    Some(rs) => rs
                                        .info
                                        .as_ref()
                                        .map(|info| {
                                            group_branches(
                                                &info.branches,
                                                rs.default_branch.as_deref(),
                                                &rs.recent_branches,
                                                &query,
crate::branch_list::sort_by_date(cx),
                                            )
                                        })
                                        .unwrap_or_default(),
                                    None => Vec::new(),
                                }
                            };
                            let on_select = cx.listener(|this, name: &String, _, cx| {
                                this.other_branch = Some(name.clone());
                                cx.notify();
                            });
                            branch_picker(
                                "create-branch-other",
                                &self.other_filter,
                                &self.other_focus,
                                groups,
                                "",
                                self.other_branch.as_deref(),
                                std::rc::Rc::new(on_select),
                                window,
                                cx,
                            )
                            .mt(SPACING())
                            .border_1()
                            .border_color(cx.ghd().box_border)
                            .rounded(BORDER_RADIUS())
                            .pt(SPACING())
                        });
                        description.push(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().mb(zpx(5.)).child("Create branch based on…"))
                                .child(div().flex().flex_col().children(
                                    options.into_iter().enumerate().map(
                                        |(ix, (id, title, detail, point))| {
                                            segmented_option(
                                                id,
                                                title,
                                                detail,
                                                selected == point,
                                                ix == 0,
                                                ix == last,
                                                cx,
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.start_point = point;
                                                cx.notify();
                                            }))
                                        },
                                    ),
                                ))
                                .children(picker)
                                .into_any_element(),
                        );
                    } else {
                        let is_default = default_branch.as_deref() == Some(current_name.as_str());
                        let mut parts: Vec<Inline> = vec![
                            "Your new branch will be based on your currently checked out branch (".into(),
                            ref_chip(current_name.clone(), cx).into_any_element().into(),
                            "). ".into(),
                        ];
                        if is_default {
                            parts.push(ref_chip(current_name.clone(), cx).into_any_element().into());
                            // `defaultBranchLink`
                            parts.push(" is the ".into());
                            parts.push(
                                crate::widgets::link_button(
                                    "create-branch-default-link",
                                    "default branch",
                                    cx,
                                )
                                .on_click(|_, _, cx| {
                                    corvene_core::Dispatcher::open_url(
                                        "https://help.github.com/articles/setting-the-default-branch/",
                                        cx,
                                    )
                                })
                                .into_any_element()
                                .into(),
                            );
                            parts.push(" for your repository.".into());
                        }
                        description.push(paragraph(parts).into_any_element());
                    }
                }
                Tip::Unknown => {}
            }
        }
        // GHD `renderForkLink`
        let forked = self
            .state
            .read(cx)
            .repository(self.repo)
            .and_then(|r| r.github.as_ref())
            .is_some_and(|g| g.parent.is_some());
        if forked && target_commit.is_none() && matches!(tip, Tip::Valid { .. }) {
            let repo = self.repo;
            description.push(
                paragraph(vec![
                    "Your default branch source is determined by your ".into(),
                    crate::widgets::link_button(
                        "create-branch-fork-settings",
                        "fork behavior settings",
                        cx,
                    )
                    .on_click(move |_, _, cx| {
                        Dispatcher::show_popup(corvene_core::Popup::ChooseForkSettings { repo }, cx)
                    })
                    .into_any_element()
                    .into(),
                    ".".into(),
                ])
                .into_any_element(),
            );
        }
        let _ = current;
        let rule_notice = branch_name_rule_notice(self.state.read(cx), self.repo, &name, cx);
        let rule_error = rule_notice.as_ref().is_some_and(|(_, error)| *error);
        let disabled = name.is_empty() || exists || reserved || needs_pick || rule_error;

        let repo = self.repo;
        let unborn = matches!(tip, Tip::Unborn { .. });
        let content = div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                // `RefNameTextBox`: label, 3.33 px, the box
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACING_THIRD())
                    .child("Name")
                    .child(text_box("branch-name", &self.name, None, window, cx))
                    .children(ref_name_notice_with(
                        &raw,
                        &branch_forbidden_chars(cx),
                        "created",
                        cx,
                    )),
            )
            .when(exists, |d| {
                d.child(crate::widgets::input_error(
                    format!("A branch named {name} already exists."),
                    cx,
                ))
            })
            .when(reserved, |d| {
                d.child(crate::widgets::input_error(
                    reserved_head_message(&name),
                    cx,
                ))
            })
            .children(rule_notice.map(|(row, _)| row))
            .when_some(remote_warning, |d, warning| {
                d.child(branch_name_warning(warning, cx))
            })
            .children(description);
        let name_for_ok = name.clone();
        // `787-commit-to-new-branch`
        let then: Option<NameThen> = match &self.purpose {
            Purpose::Branch => None,
            Purpose::Commit {
                summary,
                description,
            } => {
                let (summary, description) = (summary.clone(), description.clone());
                Some(Box::new(move |name, cx| {
                    Dispatcher::commit_to_new_branch(
                        repo,
                        name,
                        summary.clone(),
                        description.clone(),
                        cx,
                    )
                }))
            }
            Purpose::FromCommits(plan) => {
                let commits = plan.commits.clone();
                let move_back = plan.move_back.clone().filter(|_| self.remove_from_current);
                Some(Box::new(move |name, cx| {
                    Dispatcher::create_branch_from_commits(
                        repo,
                        name,
                        commits.clone(),
                        move_back.clone(),
                        cx,
                    )
                }))
            }
        };
        let (title, ok_label): (&'static str, SharedString) = match &self.purpose {
            Purpose::Commit { .. } => (
                mac_or("Commit to New Branch", "Commit to new branch"),
                mac_or("Create Branch and Commit", "Create branch and commit").into(),
            ),
            Purpose::FromCommits(_) if self.remove_from_current => (
                mac_or("Create Branch from Commits", "Create branch from commits"),
                mac_or(
                    "Create Branch and Move Commits",
                    "Create branch and move commits",
                )
                .into(),
            ),
            Purpose::FromCommits(_) => (
                mac_or("Create Branch from Commits", "Create branch from commits"),
                mac_or("Create Branch", "Create branch").into(),
            ),
            Purpose::Branch if cherry_pick => (
                mac_or("Cherry-pick to New Branch", "Cherry-pick to new branch"),
                mac_or(
                    "Create Branch and Cherry-pick",
                    "Create branch and cherry-pick",
                )
                .into(),
            ),
            Purpose::Branch => (
                mac_or("Create a Branch", "Create a branch"),
                mac_or("Create Branch", "Create branch").into(),
            ),
        };
        dialog(
            "dialog-create-branch",
            title,
            content,
            vec![
                DialogButton {
                    id: "create-branch-cancel",
                    label: "Cancel".into(),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(close),
                },
                DialogButton {
                    id: "create-branch-ok",
                    label: ok_label,
                    primary: true,
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        if disabled {
                            return;
                        }
                        if let Some(then) = &then {
                            Dispatcher::close_popup(cx);
                            then(name_for_ok.clone(), cx);
                            return;
                        }
                        if cherry_pick {
                            Dispatcher::cherry_pick_to_new_branch(
                                repo,
                                name_for_ok.clone(),
                                start_point.clone(),
                                cx,
                            );
                            return;
                        }
                        Dispatcher::close_popup(cx);
                        Dispatcher::create_branch(
                            repo,
                            name_for_ok.clone(),
                            start_point.clone(),
                            unborn,
                            cx,
                        );
                    }),
                },
            ],
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

pub struct RenameBranchDialog {
    state: Entity<AppState>,
    repo: u64,
    branch: String,
    name: Entity<InputState>,
    /// The close button's focus ring, until a mouse press.
    close_focus_visible: bool,
    rules_check: BranchRulesCheck,
}

impl RenameBranchDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        name.update(cx, |s, cx| s.set_value(branch.clone(), window, cx));
        // GHD `onNameChange`: only an edited name is checked
        cx.observe(&name, |this: &mut Self, name, cx| {
            let new_name = sanitize_branch_name(name.read(cx).value().as_ref(), cx);
            this.rules_check.schedule(this.repo, new_name, cx);
            cx.notify();
        })
        .detach();
        // `872-rename-branch-focuses-name`: the name box takes the focus with
        // the name selected (GHD `focusCloseButtonOnOpen` focuses the close
        // button)
        let focus_name = state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::RENAME_BRANCH_FOCUSES_NAME);
        if focus_name {
            let handle = name.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            name.update(cx, |input, cx| input.select_all(window, cx));
        }
        let rules_check = BranchRulesCheck {
            name: sanitize_branch_name(&branch, cx),
            _task: None,
        };
        Self {
            state,
            repo,
            branch,
            rules_check,
            name,
            close_focus_visible: !focus_name,
        }
    }
}

impl Render for RenameBranchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let t = cx.ghd();
        let raw = self.name.read(cx).value().to_string();
        let new_name = sanitize_branch_name(&raw, cx);
        let (remote_warning, existing) = {
            let s = self.state.read(cx);
            let info = s.repo_states.get(&self.repo).and_then(|r| r.info.as_ref());
            let branch = info.and_then(|i| i.branches.iter().find(|b| b.name == self.branch));
            (
                branch.and_then(render_branch_has_remote_warning),
                info.map(|i| {
                    i.branches
                        .iter()
                        .filter(|b| b.kind == BranchKind::Local)
                        .map(|b| b.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            )
        };
        let exists = new_name != self.branch && existing.contains(&new_name);
        // GHD: disabled only while the name is empty or invalid (renaming to
        // the same name is allowed); `head` is refused (`reserved_head_name`)
        let reserved = reserved_head_name(&new_name, cx);
        let rule_notice = branch_name_rule_notice(self.state.read(cx), self.repo, &new_name, cx);
        let rule_error = rule_notice.as_ref().is_some_and(|(_, error)| *error);
        let disabled = new_name.is_empty() || exists || reserved || rule_error;
        let (repo, old) = (self.repo, self.branch.clone());
        let content = div()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.close_focus_visible {
                        this.close_focus_visible = false;
                        cx.notify();
                    }
                }),
            )
            .flex()
            .flex_col()
            .gap(SPACING())
            .when_some(remote_warning, |d, warning| {
                d.child(branch_name_warning(warning, cx))
            })
            .child(
                // `.ref-name-text-box`: label, 3.33 px, the box; 10 px below
                // (kept inside the content's padding, as in GHD); a rule
                // notice below takes the margin over
                div()
                    .when(rule_notice.is_none(), |d| d.mb(SPACING()))
                    .flex()
                    .flex_col()
                    .gap(SPACING_THIRD())
                    .child("Name")
                    .child(text_box("rename-branch-name", &self.name, None, window, cx))
                    .children(ref_name_notice_with(
                        &raw,
                        &branch_forbidden_chars(cx),
                        "created",
                        cx,
                    )),
            )
            .children(rule_notice.map(|(row, _)| div().mb(SPACING()).child(row)))
            .when(exists, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(zpx(4.))
                        .text_color(t.form_error_text)
                        .child("A branch named")
                        .child(ref_chip(new_name.clone(), cx))
                        .child("already exists"),
                )
            })
            .when(reserved, |d| {
                d.child(
                    div()
                        .text_color(t.form_error_text)
                        .child(reserved_head_message(&new_name)),
                )
            });
        let name_for_ok = new_name.clone();
        // `focusCloseButtonOnOpen`: the close button, not the name, has focus
        crate::dialog::dialog_with_frame(
            "dialog-rename-branch",
            mac_or("Rename Branch", "Rename branch"),
            content,
            vec![
                DialogButton {
                    id: "rename-cancel",
                    label: "Cancel".into(),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(close),
                },
                DialogButton {
                    id: "rename-ok",
                    label: format!("Rename {}", self.branch).into(),
                    primary: true,
                    disabled,
                    on_click: Box::new(move |_, cx| {
                        if disabled {
                            return;
                        }
                        Dispatcher::close_popup(cx);
                        Dispatcher::rename_branch(repo, old.clone(), name_for_ok.clone(), cx);
                    }),
                },
            ],
            DialogFrame {
                focus_close: self.close_focus_visible,
                ..DialogFrame::default()
            },
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

pub struct DeleteBranchDialog {
    state: Entity<AppState>,
    repo: u64,
    branch: String,
    include_remote: bool,
}

impl DeleteBranchDialog {
    pub fn new(state: Entity<AppState>, repo: u64, branch: String, cx: &mut Context<Self>) -> Self {
        if state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::DELETE_BRANCH_WARNINGS)
        {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            Dispatcher::preview_delete_branch(repo, branch.clone(), cx);
        }
        Self {
            state,
            repo,
            branch,
            include_remote: false,
        }
    }
}

impl Render for DeleteBranchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let t = cx.ghd();
        // `860-delete-branch-warnings`: what the deletion would lose
        let warnings: Vec<String> = {
            let s = self.state.read(cx);
            s.repo_states
                .get(&self.repo)
                .and_then(|r| r.delete_branch_preview.as_ref())
                .filter(|p| {
                    p.branch == self.branch
                        && s.flags
                            .bool(corvene_core::flags::ids::DELETE_BRANCH_WARNINGS)
                })
                .map(|p| {
                    let mut out = Vec::new();
                    if p.unmerged_commits > 0 {
                        let n = p.unmerged_commits;
                        out.push(format!(
                            "{n} {} on this branch {} not on {}. {} will be lost unless {} on another branch.",
                            if n == 1 { "commit" } else { "commits" },
                            if n == 1 { "is" } else { "are" },
                            match p.compared_to.as_slice() {
                                [] => String::new(),
                                [one] => one.clone(),
                                [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
                            },
                            if n == 1 { "It" } else { "They" },
                            if n == 1 { "it is" } else { "they are" },
                        ));
                    }
                    if p.has_stash {
                        out.push(
                            "This branch has stashed changes, which will no longer be shown once the branch is deleted."
                                .to_string(),
                        );
                    }
                    out
                })
                .unwrap_or_default()
        };
        // `870-delete-remote-names-upstream`: the checkbox names the
        // upstream, and is not offered for the remote's default branch (a
        // local branch tracking origin/main would delete origin/main)
        let names_upstream = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::DELETE_REMOTE_NAMES_UPSTREAM);
        let remote_upstream = {
            let s = self.state.read(cx);
            let rs = s.repo_states.get(&self.repo);
            let info = rs.and_then(|r| r.info.as_ref());
            let upstream = info
                .and_then(|i| i.branches.iter().find(|b| b.name == self.branch))
                .and_then(|b| b.upstream_short().map(|u| u.to_string()))
                .filter(|u| {
                    info.is_some_and(|i| {
                        i.branches
                            .iter()
                            .any(|b| b.kind == BranchKind::Remote && &b.name == u)
                    })
                });
            let is_remote_default = |u: &str| {
                let Some(default) = rs.and_then(|r| r.default_branch.as_deref()) else {
                    return false;
                };
                let default_upstream = info
                    .and_then(|i| {
                        i.branches
                            .iter()
                            .find(|b| b.name == default && b.kind == BranchKind::Local)
                    })
                    .and_then(|b| b.upstream_short());
                default_upstream == Some(u)
                    || u.split_once('/').is_some_and(|(_, name)| name == default)
            };
            upstream.filter(|u| !(names_upstream && is_remote_default(u)))
        };
        let exists_on_remote = remote_upstream.is_some();
        let remote_label = match remote_upstream.as_deref().filter(|_| names_upstream) {
            Some(upstream) => format!("Yes, delete {upstream} on the remote"),
            None => "Yes, delete this branch on the remote".to_string(),
        };
        let (repo, name, include_remote) = (self.repo, self.branch.clone(), self.include_remote);
        let content = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(SPACING())
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(zpx(4.))
                    .child("Delete branch")
                    .child(ref_chip(self.branch.clone(), cx))
                    .child("?"),
            )
            .children(warnings.into_iter().map(|warning| {
                div()
                    .mb(SPACING())
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(SPACING_HALF())
                    .child(octicon(Octicon::Alert, t.dialog_warning))
                    .child(div().flex_1().min_w_0().child(warning))
            }))
            // the last paragraph has no bottom margin
            .child(
                div()
                    .when(exists_on_remote, |d| d.mb(SPACING()))
                    .child("This action cannot be undone."),
            )
            .when(exists_on_remote, |d| {
                d.child(div().mb(SPACING()).font_weight(FontWeight::SEMIBOLD).child(
                    "The branch also exists on the remote, do you wish to delete it there as well?",
                ))
                .child(
                    div()
                        .id("delete-remote-row")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.include_remote = !this.include_remote;
                            cx.notify();
                        }))
                        .child(checkbox(
                            "delete-remote-box",
                            self.include_remote,
                            false,
                            cx,
                        ))
                        .child(remote_label),
                )
            });
        // destructive: Cancel is the submit button, which gets the focus
        // (`focusFirstSuitableChild`)
        crate::dialog::dialog_with_kind_framed(
            "dialog-delete-branch",
            DialogKind::Warning,
            mac_or("Delete Branch", "Delete branch"),
            content,
            vec![
                DialogButton {
                    id: "delete-branch-cancel",
                    label: "Cancel".into(),
                    primary: true,
                    disabled: false,
                    on_click: Box::new(close),
                },
                DialogButton {
                    id: "delete-branch-ok",
                    label: crate::dialog::confirm_label(
                        "Delete",
                        "Delete Branch",
                        "Delete branch",
                        cx,
                    ),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::delete_branch(repo, name.clone(), include_remote, cx);
                    }),
                },
            ],
            DialogFrame {
                focus_primary: !exists_on_remote,
                ..DialogFrame::default()
            },
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

pub struct StashAndSwitchBranchDialog {
    state: Entity<AppState>,
    repo: u64,
    branch: String,
    action: UncommittedChangesStrategy,
    /// `865-switch-branch-discard`: "Discard my changes" is chosen
    /// (overrides `action`).
    discard: bool,
    /// `1313-changelists`: per changelist, whether it stays stashed on the
    /// current branch (`true`) or travels, when chosen apart from `action`.
    list_choices: std::collections::HashMap<u64, bool>,
}

impl StashAndSwitchBranchDialog {
    pub fn new(state: Entity<AppState>, repo: u64, branch: String) -> Self {
        Self {
            state,
            repo,
            branch,
            action: UncommittedChangesStrategy::StashOnCurrentBranch,
            discard: false,
            list_choices: Default::default(),
        }
    }

    /// `1313-changelists`: the lists with changed files, each with whether
    /// it stays (its own choice, else the main one).
    fn list_rows(&self, cx: &App) -> Vec<(u64, String, usize, bool)> {
        let s = self.state.read(cx);
        let status = s
            .repo_states
            .get(&self.repo)
            .and_then(|rs| rs.status.as_deref());
        let main_leaves = self.action == UncommittedChangesStrategy::StashOnCurrentBranch;
        corvene_core::changelists::lists_with_changes(s, self.repo, status)
            .into_iter()
            .map(|(id, name, n)| {
                let leaves = self.list_choices.get(&id).copied().unwrap_or(main_leaves);
                (id, name, n, leaves)
            })
            .collect()
    }
}

impl Render for StashAndSwitchBranchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let t = cx.ghd();
        let (current, has_stash, behind) = {
            let s = self.state.read(cx);
            let rs = s.repo_states.get(&self.repo);
            (
                rs.and_then(|r| r.info.as_ref())
                    .and_then(|i| i.current_branch())
                    .map(|b| b.name.clone())
                    .unwrap_or_default(),
                rs.is_some_and(|r| r.desktop_stash().is_some()),
                // `1207-switch-warns-target-behind`
                rs.and_then(|r| r.switch_target_behind.clone())
                    .filter(|(branch, _, _)| *branch == self.branch)
                    .filter(|_| {
                        s.flags
                            .bool(corvene_core::flags::ids::SWITCH_WARNS_TARGET_BEHIND)
                    }),
            )
        };
        let (repo, branch, action) = (self.repo, self.branch.clone(), self.action);
        let offer_discard = self
            .state
            .read(cx)
            .flags
            .bool(corvene_core::flags::ids::SWITCH_BRANCH_DISCARD);
        let discard = offer_discard && self.discard;
        // `1313-changelists`: lists that stay or travel on their own
        let list_rows = self.list_rows(cx);
        let main_leaves = action == UncommittedChangesStrategy::StashOnCurrentBranch;
        let split = !discard
            && list_rows
                .iter()
                .any(|(_, _, _, leaves)| *leaves != main_leaves);
        let any_leaves =
            !discard && (main_leaves || list_rows.iter().any(|(_, _, _, leaves)| *leaves));
        let leave_lists: Vec<u64> = list_rows
            .iter()
            .filter(|(_, _, _, leaves)| *leaves)
            .map(|(id, _, _, _)| *id)
            .collect();
        // `dialog#stash-changes` is 450 px wide
        let content = div()
            .w(crate::theme::fit_width(408.))
            .flex()
            .flex_col()
            .gap(SPACING())
            .when(has_stash && any_leaves, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(octicon(Octicon::Alert, t.dialog_warning))
                        .child("Your current stash will be overwritten by creating a new stash"),
                )
            })
            .when(discard, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(octicon(Octicon::Alert, t.dialog_warning))
                        .child("Changes to tracked files can't be recovered once discarded"),
                )
            })
            .when_some(
                behind.filter(|_| !discard && action == UncommittedChangesStrategy::MoveToNewBranch),
                |d, (branch, upstream, count)| {
                    d.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(SPACING_HALF())
                            .child(octicon(Octicon::Alert, t.dialog_warning))
                            .child(div().flex_1().min_w_0().child(format!(
                                "{branch} is {count} {} behind {upstream}. Pull it before \
                                 bringing your changes to avoid conflicts.",
                                if count == 1 { "commit" } else { "commits" }
                            ))),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        // `legend`: 5 px padding, 3.33 px margin below
                        div()
                            .mb(zpx(5. + 10. / 3.))
                            .child("You have changes on this branch. What would you like to do with them?"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                segmented_option(
                                    "stash-leave",
                                    format!("Leave my changes on {current}"),
                                    "Your in-progress work will be stashed on this branch for you to return to later",
                                    !discard && action == UncommittedChangesStrategy::StashOnCurrentBranch,
                                    true,
                                    false,
                                    cx,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.action = UncommittedChangesStrategy::StashOnCurrentBranch;
                                    this.discard = false;
                                    this.list_choices.clear();
                                    cx.notify();
                                })),
                            )
                            .child(
                                segmented_option(
                                    "stash-bring",
                                    format!("Bring my changes to {}", self.branch),
                                    "Your in-progress work will follow you to the new branch",
                                    !discard && action == UncommittedChangesStrategy::MoveToNewBranch,
                                    false,
                                    !offer_discard,
                                    cx,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.action = UncommittedChangesStrategy::MoveToNewBranch;
                                    this.discard = false;
                                    this.list_choices.clear();
                                    cx.notify();
                                })),
                            )
                            .when(offer_discard, |d| {
                                d.child(
                                    segmented_option(
                                        "stash-discard",
                                        "Discard my changes",
                                        "Your in-progress work will be thrown away (new files go to the Trash)",
                                        discard,
                                        false,
                                        true,
                                        cx,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discard = true;
                                        cx.notify();
                                    })),
                                )
                            }),
                    ),
            )
            // `1313-changelists`: a Leave / Bring choice per changelist
            .when(!discard && !list_rows.is_empty(), |d| {
                let current = current.clone();
                let target = self.branch.clone();
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACING_HALF())
                        .child(
                            div()
                                .text_color(t.text_secondary)
                                .text_size(FONT_SIZE_SM())
                                .child("Changelists can stay or travel on their own:"),
                        )
                        .children(list_rows.iter().map(|(id, name, n, leaves)| {
                            let (id, leaves) = (*id, *leaves);
                            let choice = |label: &'static str, on: bool, value: bool| {
                                div()
                                    .id(SharedString::from(format!(
                                        "switch-list-{id}-{label}"
                                    )))
                                    .px(SPACING())
                                    .h(zpx(22.))
                                    .flex()
                                    .items_center()
                                    .text_size(FONT_SIZE_SM())
                                    .border_1()
                                    .border_color(t.secondary_button_border)
                                    .cursor_pointer()
                                    .map(|d| {
                                        if on {
                                            d.bg(t.box_selected_active_background)
                                                .text_color(t.box_selected_active_text)
                                        } else {
                                            d.bg(t.secondary_button_background)
                                                .text_color(t.secondary_button_text)
                                        }
                                    })
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.list_choices.insert(id, value);
                                        cx.notify();
                                    }))
                            };
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(SPACING())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(format!(
                                            "{name} ({n} {})",
                                            if *n == 1 { "file" } else { "files" }
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .child(
                                            choice("Leave", leaves, true)
                                                .rounded_l(BORDER_RADIUS())
                                                .border_r_0(),
                                        )
                                        .child(
                                            choice("Bring", !leaves, false)
                                                .rounded_r(BORDER_RADIUS()),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(FONT_SIZE_SM())
                                        .text_color(t.text_secondary)
                                        .child(if leaves {
                                            format!("stays on {current}")
                                        } else {
                                            format!("goes to {target}")
                                        }),
                                )
                        })),
                )
            });
        dialog(
            "dialog-stash-and-switch",
            mac_or("Switch Branch", "Switch branch"),
            content,
            vec![
                DialogButton {
                    id: "switch-cancel",
                    label: "Cancel".into(),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(close),
                },
                DialogButton {
                    id: "switch-ok",
                    label: if discard {
                        mac_or("Discard Changes and Switch", "Discard changes and switch").into()
                    } else {
                        mac_or("Switch Branch", "Switch branch").into()
                    },
                    primary: true,
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        if discard {
                            Dispatcher::discard_all_and_checkout(repo, branch.clone(), cx);
                        } else if split {
                            // `1313-changelists`: some lists stay, the rest travel
                            Dispatcher::checkout_branch_splitting_changelists(
                                repo,
                                branch.clone(),
                                leave_lists.clone(),
                                main_leaves,
                                cx,
                            );
                        } else {
                            Dispatcher::checkout_branch(repo, branch.clone(), Some(action), cx);
                        }
                    }),
                },
            ],
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

pub struct ConfirmOverwriteStashDialog {
    repo: u64,
    /// `branchToCheckout`: `None` for Stash All Changes.
    branch: Option<String>,
}

impl ConfirmOverwriteStashDialog {
    pub fn new(repo: u64, branch: Option<String>) -> Self {
        Self { repo, branch }
    }
}

impl Render for ConfirmOverwriteStashDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, branch) = (self.repo, self.branch.clone());
        // Corvene (`776-stash-add-to-existing`): or fold the changes in
        let add = Dispatcher::add_to_stash_available(AppState::global(cx).read(cx));
        let mut buttons = vec![DialogButton {
            id: "overwrite-cancel",
            label: "Cancel".into(),
            primary: true,
            disabled: false,
            on_click: Box::new(close),
        }];
        if add {
            let branch = branch.clone();
            buttons.push(DialogButton {
                id: "overwrite-add",
                label: mac_or("Add to Stash", "Add to stash").into(),
                primary: false,
                disabled: false,
                on_click: Box::new(move |_, cx| {
                    Dispatcher::close_popup(cx);
                    match &branch {
                        Some(branch) => {
                            Dispatcher::add_to_stash_and_checkout(repo, branch.clone(), cx)
                        }
                        None => Dispatcher::add_to_stash(repo, cx),
                    }
                }),
            });
        }
        buttons.push(DialogButton {
            id: "overwrite-ok",
            label: "Overwrite".into(),
            primary: false,
            disabled: false,
            on_click: Box::new(move |_, cx| {
                Dispatcher::close_popup(cx);
                match &branch {
                    Some(branch) => Dispatcher::checkout_branch(
                        repo,
                        branch.clone(),
                        Some(UncommittedChangesStrategy::StashOnCurrentBranch),
                        cx,
                    ),
                    None => Dispatcher::create_stash_for_current_branch(repo, false, cx),
                }
            }),
        });
        let content = div()
            .flex()
            .flex_col()
            .gap(SPACING())
            .child(
                "Are you sure you want to proceed? This will overwrite your existing stash with your current changes.",
            )
            .when(add, |d| {
                d.child(
                    "Add to Stash keeps both: your changes join the existing stash, unless the two change the same lines.",
                )
            });
        dialog_with_kind(
            "dialog-overwrite-stash",
            DialogKind::Warning,
            mac_or("Overwrite Stash?", "Overwrite stash?"),
            content,
            buttons,
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

/// Corvene addition (`774-stash-conflict-flow`): every conflict a stash
/// restore left is resolved; drop the entry git kept, or keep it.
pub struct DropKeptStashDialog {
    repo: u64,
    stash: corvene_core::StashEntry,
}

impl DropKeptStashDialog {
    pub fn new(repo: u64, stash: corvene_core::StashEntry) -> Self {
        Self { repo, stash }
    }
}

impl Render for DropKeptStashDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, sha) = (self.repo, self.stash.sha.clone());
        let which = match &self.stash.branch {
            Some(branch) => format!("the stash of {branch}"),
            None => format!("the stash \"{}\"", self.stash.message),
        };
        dialog_with_kind(
            "dialog-drop-kept-stash",
            DialogKind::Warning,
            mac_or("Drop the Stash?", "Drop the stash?"),
            div().w(crate::theme::fit_width(408.)).child(format!(
                "Every conflict from restoring {which} is resolved. Git kept the stash in case \
                 the restore went wrong; drop it now, or keep it to restore again later."
            )),
            vec![
                DialogButton {
                    id: "drop-kept-stash-keep",
                    label: mac_or("Keep Stash", "Keep stash").into(),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(close),
                },
                DialogButton {
                    id: "drop-kept-stash-drop",
                    label: mac_or("Drop Stash", "Drop stash").into(),
                    primary: true,
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::drop_stash_entry(repo, sha.clone(), cx);
                    }),
                },
            ],
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

/// Corvene addition (`864-confirm-branch-switch`): "Switch to <branch>?"
/// before a checkout started from the branch list.
pub struct ConfirmSwitchBranchDialog {
    repo: u64,
    branch: String,
}

impl ConfirmSwitchBranchDialog {
    pub fn new(repo: u64, branch: String) -> Self {
        Self { repo, branch }
    }
}

impl Render for ConfirmSwitchBranchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let (repo, branch) = (self.repo, self.branch.clone());
        dialog(
            "dialog-confirm-switch-branch",
            mac_or("Switch Branch?", "Switch branch?"),
            paragraph(vec![
                "Switch to ".into(),
                ref_chip(self.branch.clone(), cx).into_any_element().into(),
                "?".into(),
            ]),
            vec![
                DialogButton {
                    id: "confirm-switch-cancel",
                    label: "Cancel".into(),
                    primary: false,
                    disabled: false,
                    on_click: Box::new(close),
                },
                DialogButton {
                    id: "confirm-switch-ok",
                    label: mac_or("Switch Branch", "Switch branch").into(),
                    primary: true,
                    disabled: false,
                    on_click: Box::new(move |_, cx| {
                        Dispatcher::close_popup(cx);
                        Dispatcher::checkout_branch(repo, branch.clone(), None, cx);
                    }),
                },
            ],
            close,
            window,
            cx,
        )
    }
}

// ---------------------------------------------------------------------------

/// `MergeChooseBranchDialog`: pick a branch, preview the commit count, merge.
pub struct MergeBranchDialog {
    state: Entity<AppState>,
    repo: u64,
    squash: bool,
    filter: Entity<InputState>,
    /// The branch list takes focus when a row is pressed.
    list_focus: FocusHandle,
    selected: Option<String>,
    /// Squash and merge's commit message (flag `837`).
    summary: Entity<InputState>,
    description: Entity<TextareaState>,
}

impl MergeBranchDialog {
    pub fn new(
        state: Entity<AppState>,
        repo: u64,
        squash: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // `FilterList` autofocuses its filter box
        let handle = filter.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        let summary =
            cx.new(|cx| InputState::new(window, cx).placeholder("Commit summary (optional)"));
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Description")
        });
        cx.observe(&summary, |_, _, cx| cx.notify()).detach();
        cx.observe(&description, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            repo,
            squash,
            filter,
            list_focus: cx.focus_handle().tab_stop(true),
            selected: None,
            summary,
            description,
        }
    }
}

impl Render for MergeBranchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = |_: &mut Window, cx: &mut App| Dispatcher::close_popup(cx);
        let t = cx.ghd();
        let query = self.filter.read(cx).value().trim().to_string();
        let (current, groups, preview) = {
            let s = self.state.read(cx);
            let rs = s.repo_states.get(&self.repo);
            let info = rs.and_then(|r| r.info.as_ref());
            let current = info
                .and_then(|i| i.current_branch())
                .map(|b| b.name.clone())
                .unwrap_or_default();
            let groups = match (info, rs) {
                (Some(info), Some(rs)) => group_branches(
                    &info.branches,
                    rs.default_branch.as_deref(),
                    &rs.recent_branches,
                    &query,
                    crate::branch_list::sort_by_date(cx),
                ),
                _ => Vec::new(),
            };
            (current, groups, rs.and_then(|r| r.merge_preview.clone()))
        };
        let repo = self.repo;
        let selected = self.selected.clone();
        let preview = preview.filter(|p| Some(&p.branch) == selected.as_ref());
        // `getDialogTitle`: light "Merge into" with the branch in <strong>
        // (a step bolder: regular), no header border
        let plain_title = if self.squash {
            format!("Squash and Merge into {current}")
        } else {
            format!("Merge into {current}")
        };
        let title = div()
            .flex()
            .flex_row()
            .font_weight(FontWeight::LIGHT)
            .child(if self.squash {
                "Squash and Merge into\u{a0}"
            } else {
                "Merge into\u{a0}"
            })
            .child(div().font_weight(FontWeight::NORMAL).child(
                corvene_core::notifications::truncate_with_ellipsis(&current, 40),
            ));
        let on_select = cx.listener(move |this, name: &String, _, cx| {
            this.selected = Some(name.clone());
            Dispatcher::preview_merge(repo, name.clone(), cx);
            cx.notify();
        });
        let list = branch_picker(
            "merge",
            &self.filter,
            &self.list_focus,
            groups,
            &current,
            selected.as_deref(),
            std::rc::Rc::new(on_select),
            window,
            cx,
        );
        // `.merge-status-component` (`MergeStatusHeader`)
        // `.merge-info strong`: bold, in the text colour
        let bold = |text: String| {
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(t.text)
                .child(text)
        };
        let status: Option<AnyElement> =
            selected.as_ref().filter(|b| **b != current).map(|branch| {
                let row = || div().flex().flex_row().flex_wrap().justify_center();
                let (icon, color, message): (Octicon, Hsla, AnyElement) = match &preview {
                    None => (
                        Octicon::DotFill,
                        t.color_modified,
                        div()
                            .child("Checking for ability to merge automatically...")
                            .into_any_element(),
                    ),
                    Some(p) if p.mergeability == Some(Mergeability::Invalid) => (
                        Octicon::X,
                        t.color_deleted,
                        div()
                            .child("Unable to merge unrelated histories in this repository")
                            .into_any_element(),
                    ),
                    Some(p) if p.commits == 0 => (
                        Octicon::Check,
                        t.color_new,
                        row()
                            .child(bold(current.clone()))
                            .child("\u{a0}is already up to date with\u{a0}")
                            .child(bold(branch.clone()))
                            .into_any_element(),
                    ),
                    Some(p) => {
                        let commits = format!(
                            "{} {}",
                            p.commits,
                            if p.commits == 1 { "commit" } else { "commits" }
                        );
                        match p.mergeability {
                            Some(Mergeability::Conflicts(n)) => (
                                Octicon::Alert,
                                t.color_modified,
                                row()
                                    .child("There will be\u{a0}")
                                    .child(bold(format!(
                                        "{n} conflicted {}",
                                        if n == 1 { "file" } else { "files" }
                                    )))
                                    .child("\u{a0}when merging\u{a0}")
                                    .child(bold(branch.clone()))
                                    .child("\u{a0}into\u{a0}")
                                    .child(bold(current.clone()))
                                    .into_any_element(),
                            ),
                            _ => (
                                Octicon::Check,
                                t.color_new,
                                row()
                                    .child("This will merge\u{a0}")
                                    .child(bold(commits))
                                    .child("\u{a0}from\u{a0}")
                                    .child(bold(branch.clone()))
                                    .child("\u{a0}into\u{a0}")
                                    .child(bold(current.clone()))
                                    .into_any_element(),
                            ),
                        }
                    }
                };
                // `.merge-status-component` in `#choose-branch`: the 20 px icon
                // row without its rule, then `.merge-info` (5 px above, 10 below)
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .h(zpx(20.))
                            .flex()
                            .justify_center()
                            .child(octicon(icon, color)),
                    )
                    .child(
                        div()
                            .mt(SPACING_HALF())
                            .mb(SPACING())
                            .text_size(FONT_SIZE())
                            .text_color(t.text_secondary)
                            .text_center()
                            .child(message),
                    )
                    .into_any_element()
            });
        // `canStartOperation`: conflicts are fine (resolved afterwards), nothing to merge is not
        let can_start = preview
            .as_ref()
            .is_some_and(|p| p.commits > 0 && p.mergeability != Some(Mergeability::Invalid))
            && selected.as_deref() != Some(current.as_str());
        let selected_for_ok = selected.clone();
        let squash = self.squash;
        // flag `837`: squash and merge takes a commit message (empty: git's
        // "Squashed commit of the following" list, as GHD)
        let message_fields = squash
            && self
                .state
                .read(cx)
                .flags
                .bool(corvene_core::flags::ids::SQUASH_MERGE_MESSAGE);
        let message = message_fields
            .then(|| {
                let summary = self.summary.read(cx).value().trim().to_string();
                let description = self.description.read(cx).value().to_string();
                (!summary.is_empty()).then(|| corvene_git::format_message(&summary, &description))
            })
            .flatten();
        let fields = message_fields.then(|| {
            div()
                .flex()
                .flex_col()
                .gap(SPACING())
                .mb(SPACING())
                .child(text_box(
                    "squash-merge-summary",
                    &self.summary,
                    None,
                    window,
                    cx,
                ))
                .child(
                    div()
                        .border_1()
                        .border_color(t.box_border_contrast)
                        .rounded(BORDER_RADIUS())
                        .bg(t.box_background)
                        .overflow_hidden()
                        .child(crate::widgets::resizable_text_area(
                            "merge-description",
                            Textarea::new(&self.description),
                            cx,
                        )),
                )
        });
        let content = div().flex().flex_col().child(list);
        let label = if squash {
            "Squash and merge"
        } else {
            "Create a merge commit"
        };
        let footer = div()
            .flex()
            .flex_col()
            .pt(SPACING())
            .px(SPACING_DOUBLE())
            .pb(SPACING_DOUBLE())
            .border_t_1()
            .border_color(t.box_border)
            .children(fields)
            .children(status)
            .child(split_button(
                "merge-ok",
                label,
                !can_start,
                move |_, cx| {
                    let Some(branch) = selected_for_ok.clone() else {
                        return;
                    };
                    Dispatcher::close_popup(cx);
                    Dispatcher::merge_branch_with_message(
                        repo,
                        branch,
                        squash,
                        message.clone(),
                        cx,
                    );
                },
                cx,
            ))
            .into_any_element();
        dialog_framed(
            "dialog-merge-branch",
            title,
            plain_title,
            content,
            DialogFrame {
                header_border: false,
                content_padding: false,
                footer: Some(footer),
                ..DialogFrame::default()
            },
            close,
            window,
            cx,
        )
    }
}

/// Callback for a branch chosen in `branch_picker`.
pub type BranchSelect = std::rc::Rc<dyn Fn(&String, &mut Window, &mut App)>;

/// The filter box + grouped branch list shared by the merge, rebase and
/// cherry-pick dialogs (`BranchList` in `#choose-branch`): a 36 px filter row
/// with a bottom border over a 264 px list of 30 px rows (20 px side padding,
/// semibold group headers, the branch icon - a check for the current branch -
/// the name, and the tip's relative date on the right). The selection draws
/// the inactive selection colours (the filter keeps focus); with nothing
/// selected the current branch shows as selected.
#[allow(clippy::too_many_arguments)]
pub fn branch_picker(
    id_prefix: &'static str,
    filter: &Entity<InputState>,
    list_focus: &FocusHandle,
    groups: Vec<crate::branch_list::BranchGroup>,
    current: &str,
    selected: Option<&str>,
    on_select: BranchSelect,
    window: &Window,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    let hover_bg = t.list_item_hover_background;
    let current = current.to_string();
    let shown_selected = selected.unwrap_or(current.as_str()).to_string();
    let focused = list_focus.is_focused(window);
    let keep_selection = crate::widgets::selection_keeps_colour_on_hover(cx);
    let (sel_bg, sel_text) = if focused {
        (t.box_selected_active_background, t.box_selected_active_text)
    } else {
        (t.box_selected_background, t.box_selected_text)
    };
    let list = div()
        .id(SharedString::from(format!("{id_prefix}-branch-list")))
        .track_focus(list_focus)
        .h(zpx(264.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .children(groups.into_iter().map(|group| {
            let current = current.clone();
            let shown_selected = shown_selected.clone();
            let on_select = on_select.clone();
            let list_focus = list_focus.clone();
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .h(zpx(30.))
                        .px(SPACING_DOUBLE())
                        .flex()
                        .items_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(FONT_SIZE())
                        .child(group.title),
                )
                .children(group.branches.into_iter().map(move |b| {
                    let is_current = b.name == current;
                    let is_selected = b.name == shown_selected;
                    let name = b.name.clone();
                    let on_select = on_select.clone();
                    let list_focus = list_focus.clone();
                    let date = b
                        .tip_time
                        .filter(|s| *s > 0)
                        .map(|s| relative(UNIX_EPOCH + Duration::from_secs(s as u64)));
                    div()
                        .id(SharedString::from(format!(
                            "{id_prefix}-branch-{}",
                            b.full_name
                        )))
                        .flex_none()
                        .h(zpx(30.))
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px(SPACING_DOUBLE())
                        .cursor_pointer()
                        .when(is_selected, |d| d.bg(sel_bg).text_color(sel_text))
                        // `.list-item:hover` outranks the inactive selection
                        .when(!(is_selected && (focused || keep_selection)), move |d| {
                            d.hover(move |s| s.bg(hover_bg))
                        })
                        // `List.onRowMouseDown`: focus the list, select at once
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            window.focus(&list_focus, cx);
                            if !is_current {
                                on_select(&name, window, cx);
                            }
                        })
                        .child(
                            octicon(
                                if is_current {
                                    Octicon::Check
                                } else {
                                    Octicon::GitBranch
                                },
                                t.text,
                            )
                            .flex_none()
                            .mr(SPACING_HALF()),
                        )
                        .child(
                            div()
                                .flex_grow(2.)
                                .min_w_0()
                                .max_w(gpui_kit::relative(0.65))
                                .mr(SPACING_HALF())
                                .truncate()
                                .text_size(FONT_SIZE())
                                .child(b.name.clone()),
                        )
                        .when_some(date, |d, date| {
                            d.child(
                                div()
                                    .flex_1()
                                    .mr(SPACING_HALF())
                                    .text_right()
                                    .whitespace_nowrap()
                                    .text_size(FONT_SIZE_SM())
                                    .line_height(zpx(16.5))
                                    .when(!is_selected, |d| d.text_color(t.text_secondary))
                                    .when(is_selected, |d| d.text_color(sel_text))
                                    .child(date),
                            )
                        })
                }))
        }))
        .with_scrollbar();
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .h(zpx(36.))
                .px(SPACING_DOUBLE())
                .pb(SPACING())
                .border_b_1()
                .border_color(t.box_border)
                .child(crate::widgets::filter_text_box(
                    SharedString::from(format!("{id_prefix}-filter")),
                    filter,
                    Some(octicon(Octicon::Search, t.text_secondary)),
                    window,
                    cx,
                )),
        )
        .child(list)
}

/// `DropdownSelectButton`: a 30 px primary invoke button beside a 28 px
/// dropdown half, full width; both dim while `disabled`.
pub fn split_button(
    id: &'static str,
    label: &'static str,
    disabled: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let t = cx.ghd();
    let hover = t.button_hover_background;
    // disabled: the group at 60 % (`crate::widgets::faded`)
    let (bg, text) = if disabled {
        (
            crate::widgets::faded(t.button_background, t.background),
            crate::widgets::faded(t.button_text, t.background),
        )
    } else {
        (t.button_background, t.button_text)
    };
    div()
        .flex()
        .flex_row()
        .h(zpx(30.))
        .child(
            div()
                .id(id)
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(bg)
                .rounded_l(BORDER_RADIUS())
                .bg(bg)
                .text_color(text)
                .text_size(FONT_SIZE())
                .when(!disabled, move |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(move |_, window, cx| on_click(window, cx))
                })
                .child(label),
        )
        .child(
            div()
                .w(zpx(28.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(bg)
                .rounded_r(BORDER_RADIUS())
                .bg(bg)
                .child(octicon(Octicon::TriangleDown, text)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn trailing_separator_alone_does_not_warn() {
        let check = |raw: &str, quiet| ref_name_warning(raw, "", quiet);
        assert!(check("feature/", false));
        assert!(!check("feature/", true));
        assert!(!check("v1.", true));
        assert!(!check("feature/x", true));
        assert!(check("my branch/", true));
        assert!(check("a..b", true));
        assert!(!check("", true));
    }

    #[::core::prelude::v1::test]
    fn forbidden_characters_become_dashes() {
        assert_eq!(
            sanitize_ref_name_with("fix#12 & more", "#&"),
            "fix-12---more"
        );
        assert_eq!(sanitize_ref_name_with("#lead/x", "#"), "lead/x");
        assert_eq!(sanitize_ref_name_with("a b", " "), "a-b");
        assert_eq!(sanitize_ref_name_with("plain", ""), "plain");
    }
}
