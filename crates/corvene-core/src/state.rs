//! `AppState`: everything the UI renders from. Lives in one GPUI entity that
//! views observe; only the `Dispatcher` mutates it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use corvene_git::GitBinary;
use corvene_store::Store;

use crate::persistence::Settings;
pub use crate::workspace::{SavedWorkspace, WorkspaceId, WorkspaceState};
use corvene_models::{
    Account, AheadBehind, Diff, GitHubRepository, Identity, Remote, Repository, RepositoryInfo,
    Section, WorkingDirectoryStatus,
};
use corvene_platform::editors::FoundEditor;
use corvene_platform::shells::FoundShell;

/// Which toolbar foldout is open (`FoldoutType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foldout {
    Repository,
    Branch,
    PushPull,
    Worktree,
}

/// What `Dispatcher::show_error` is given: plain text, or a failed git
/// command (`git`) with an optional lead sentence in `text` (Corvene's
/// plain-language explanation; empty when the dialog should find its own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorMessage {
    pub text: String,
    pub git: Option<corvene_git::GitFailure>,
}

impl ErrorMessage {
    /// A git failure with Corvene's explanation in front; `text` alone for
    /// errors git did not produce.
    pub fn explained(err: &corvene_git::GitError, explanation: Option<String>) -> Self {
        let mut message = Self::from(err);
        if let Some(explanation) = explanation {
            message.text = match message.git {
                Some(_) => explanation,
                None => format!("{explanation}\n\n{}", message.text),
            };
        }
        message
    }

    /// Everything in one string: the lead, then the command and its output
    /// (what the clipboard and the lock-file dialog get).
    pub fn full_text(&self) -> String {
        match &self.git {
            Some(git) => {
                let failed = git.summary();
                if self.text.is_empty() {
                    failed
                } else {
                    format!("{}\n\n{failed}", self.text)
                }
            }
            None => self.text.clone(),
        }
    }
}

impl From<String> for ErrorMessage {
    fn from(text: String) -> Self {
        Self { text, git: None }
    }
}

impl From<&String> for ErrorMessage {
    fn from(text: &String) -> Self {
        text.clone().into()
    }
}

impl From<&str> for ErrorMessage {
    fn from(text: &str) -> Self {
        text.to_string().into()
    }
}

impl From<&corvene_git::GitError> for ErrorMessage {
    fn from(err: &corvene_git::GitError) -> Self {
        match err.failure() {
            Some(git) => Self {
                text: String::new(),
                git: Some(git),
            },
            None => err.to_string().into(),
        }
    }
}

impl From<corvene_git::GitError> for ErrorMessage {
    fn from(err: corvene_git::GitError) -> Self {
        (&err).into()
    }
}

/// Modal dialogs (`PopupType`, the subset Corvene has so far).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Popup {
    InstallGit {
        reason: String,
    },
    Error {
        title: String,
        message: String,
        /// The failed git command when the error is one (`git` ran and
        /// exited non-zero); `message` then holds its full text.
        git: Option<corvene_git::GitFailure>,
    },
    /// GHD `PopupType.HookFailed` (`ui/hook-failed/hook-failed.tsx`): a
    /// hook failed and waits for Abort or Ignore and Continue.
    HookFailed {
        hook_name: String,
        terminal_output: String,
        reply: crate::hooks::HookFailureReply,
    },
    /// GHD `PopupType.CommitProgress` (`ui/commit-progress`): the commit's
    /// live output.
    CommitProgress {
        output: crate::hooks::CommitOutput,
    },
    /// `265-remove-stale-index-lock`: an error caused by a left-over
    /// `index.lock`, with a button to remove it.
    IndexLockExists {
        title: String,
        message: String,
        lock: PathBuf,
    },
    AddExistingRepository {
        path: Option<PathBuf>,
    },
    CreateRepository {
        path: Option<PathBuf>,
    },
    CloneRepository {
        url: Option<String>,
    },
    /// Corvene addition (flag 361): Clone a Repository reopened after a
    /// failed clone, with its URL, local path and git's error.
    CloneRepositoryRetry {
        url: String,
        path: PathBuf,
        error: String,
    },
    SignIn {
        enterprise: bool,
    },
    /// Corvene (flags 342-344): sign in to GitLab, Gitea / Forgejo or
    /// Bitbucket (`dialogs::sign_in_host`).
    SignInHost {
        kind: corvene_models::HostKind,
    },
    /// `all` selects the "Discard All Changes" wording.
    DiscardChanges {
        repo: u64,
        paths: Vec<String>,
        all: bool,
    },
    /// `InvalidatedToken`: an API call answered 401; the account was
    /// signed out and can sign in again.
    InvalidatedToken {
        account: Account,
    },
    /// `StartPullRequest`: the Preview Pull Request dialog (state in
    /// `RepositoryState::pull_request_preview`).
    StartPullRequest {
        repo: u64,
    },
    /// `CreateFork`: "Do you want to fork this repository?"
    CreateFork {
        repo: u64,
    },
    /// `CLIInstalled`: the command line tool was linked at `path`.
    CLIInstalled {
        path: PathBuf,
    },
    /// `MoveToApplicationsFolder`: offered at launch outside /Applications.
    MoveToApplicationsFolder,
    /// `Acknowledgements`: License and Open Source Notices.
    Acknowledgements,
    /// Corvene addition (flag 206): pick repositories from GitHub Desktop's
    /// list to add.
    ImportFromGitHubDesktop,
    /// Corvene addition (flag 269): tick repositories to remove at once;
    /// `ticked` starts ticked (the row the context menu was opened on).
    RemoveRepositories {
        ticked: Option<u64>,
    },
    /// Corvene addition (flag 455): Repository › Add License….
    AddLicense {
        repo: u64,
    },
    /// `DiscardChangesRetry`: new files a discard could not move to the
    /// Trash; delete them permanently?
    ConfirmDeleteUntrashable {
        repo: u64,
        paths: Vec<String>,
    },
    /// Corvene addition (`768-ignore-custom-pattern`): the changes file menu's "Ignore with
    /// Pattern…", prefilled with `pattern`.
    IgnoreWithPattern {
        repo: u64,
        pattern: String,
    },
    /// Corvene addition: crash reports left by the previous session (newest
    /// first), with "Save crash reports locally" on.
    CrashReportFound {
        reports: Vec<PathBuf>,
    },
    /// `ReleaseNotes`: what's new in the running version.
    ReleaseNotes {
        summary: crate::release_notes::ReleaseSummary,
    },
    /// `UpstreamAlreadyExists`: the fork's `upstream` remote points elsewhere.
    UpstreamAlreadyExists {
        repo: u64,
        existing_url: String,
    },
    /// `ChooseForkSettings`: "How are you planning to use this fork?"
    ChooseForkSettings {
        repo: u64,
    },
    /// `PushProtectionError`: secrets the server refused; `bypassed` lists
    /// the placeholder ids already allowed through.
    PushProtectionError {
        repo: u64,
        secrets: Vec<corvene_models::SecretScanResult>,
        bypassed: Vec<String>,
    },
    /// `BypassPushProtection`: why a secret gets pushed anyway.
    BypassPushProtection {
        repo: u64,
        secret: corvene_models::SecretScanResult,
        secrets: Vec<corvene_models::SecretScanResult>,
        bypassed: Vec<String>,
    },
    /// `PushRejectedDueToMissingWorkflowScope`
    PushRejectedDueToMissingWorkflowScope {
        repo: u64,
        rejected_path: String,
    },
    /// `SAMLReauthRequired`
    SAMLReauthRequired {
        repo: u64,
        organization: String,
        endpoint: String,
        retry: Option<RetryAction>,
    },
    /// `CreateTutorialRepository`: "Start tutorial" for `account`, with the
    /// creation progress (title, percent, detail) once it runs.
    CreateTutorialRepository {
        account: Account,
        progress: Option<(String, u8, Option<String>)>,
    },
    /// `ConfirmExitTutorial`
    ConfirmExitTutorial,
    /// Corvene on Android: git settings offered through the
    /// `importGitConfig` link (`git_config_import`), and how many more were
    /// left out.
    ImportGitConfig {
        settings: Vec<(String, String)>,
        skipped: usize,
    },
    /// Corvene on Android: the passphrase of the SSH key picked in Options ›
    /// Integrations (`Dispatcher::import_ssh_key`); `wrong` after one that
    /// did not open it.
    SshKeyPassphrase {
        path: std::path::PathBuf,
        wrong: bool,
    },
    /// Corvene `1223-checkout-from-fork`: Branch › Check Out from Fork…
    /// for `repo`, the owner field starting as `input`.
    CheckoutFromFork {
        repo: u64,
        input: String,
    },
    /// Corvene `350-ssh-key-helper`: Settings › Integrations' Create SSH
    /// Key… on the desktop (passphrase, ssh-agent, upload).
    CreateSshKey,
    /// Corvene `350-ssh-key-helper`: the account `login` at `endpoint` may
    /// not add SSH keys (no `write:public_key`); sign in again to allow it
    /// and add the key as `title`.
    SshKeyNeedsScope {
        endpoint: String,
        login: String,
        title: String,
    },
    /// `TestNotifications`: post sample pull request notifications for
    /// `repo` (debug builds).
    TestNotifications {
        repo: u64,
    },
    /// `CICheckRunRerun`: re-run (failed) checks of the PR head ref.
    CICheckRunRerun {
        repo: u64,
        github: GitHubRepository,
        checks: Vec<corvene_models::RefCheck>,
        git_ref: String,
        failed_only: bool,
    },
    /// Corvene (`347-actions-job-logs`): the log of an Actions job from the
    /// check-run popover; `step` is the step to scroll to (the API's name),
    /// `None` the failure.
    ActionsJobLog {
        /// `None` for a repository that is not cloned (`351-actions`).
        repo: Option<u64>,
        github: GitHubRepository,
        check: corvene_models::RefCheck,
        step: Option<String>,
    },
    /// Corvene (`351-actions`): the GitHub repository's Actions (workflows,
    /// runs, jobs); `repo` is the local clone, when there is one. The view's
    /// state is `AppState::actions`.
    Actions {
        github: GitHubRepository,
        repo: Option<u64>,
    },
    /// Corvene (`351-actions`): Run workflow for a `workflow_dispatch`
    /// workflow of the open Actions view.
    RunWorkflow {
        github: GitHubRepository,
        workflow: u64,
    },
    /// `PullRequestReview`: a review on one of the user's pull requests
    /// (GHD shows it from a notification). `should_*` pick the OK button:
    /// switch repository and/or check out the PR branch.
    PullRequestReview {
        repo: u64,
        pull_request: corvene_models::PullRequest,
        review: corvene_github::api::ApiPullRequestReview,
        should_checkout_branch: bool,
        should_change_repository: bool,
    },
    /// `PullRequestComment`: a comment on one of the user's pull requests.
    PullRequestComment {
        repo: u64,
        pull_request: corvene_models::PullRequest,
        comment: corvene_github::api::ApiIssueComment,
        should_checkout_branch: bool,
        should_change_repository: bool,
    },
    /// `PullRequestChecksFailed`: checks failed on one of the user's pull
    /// requests.
    PullRequestChecksFailed {
        repo: u64,
        pull_request: corvene_models::PullRequest,
        checks: Vec<corvene_models::RefCheck>,
        should_change_repository: bool,
    },
    /// `UnknownAuthors`: co-author handles that could not be resolved;
    /// "Commit Anyway" commits with the known ones only.
    UnknownAuthors {
        repo: u64,
        usernames: Vec<String>,
        summary: String,
        description: String,
    },
    /// `OversizedFiles`: included files over 100 MiB that Git LFS does not
    /// track; "Commit Anyway" commits them. `lfs_patterns`: what Corvene's
    /// "Track … in Git LFS" button tracks (`784-suggest-lfs-tracking`, Git
    /// LFS installed; empty otherwise).
    OversizedFiles {
        repo: u64,
        files: Vec<String>,
        summary: String,
        description: String,
        lfs_patterns: Vec<String>,
        /// Corvene `1308-ignore-oversized-files`: `Some` (the files of
        /// `files` the index tracks) offers "Add to .gitignore", and "Ignore
        /// and Untrack" when it is not empty.
        ignore_tracked: Option<Vec<String>>,
        /// The commit's checks so far (Commit Anyway goes on with them).
        checks: crate::commit_checks::CommitChecks,
    },
    /// Corvene `785-embedded-repo-commit`: untracked folders that are git
    /// repositories: add them as submodules (with an `origin`) or pointers?
    /// `commit`: the summary, description and checks of the commit that
    /// goes on afterwards (`None` from the changes list's menu).
    AddEmbeddedRepositories {
        repo: u64,
        repositories: Vec<corvene_git::EmbeddedRepository>,
        commit: Option<(String, String, crate::commit_checks::CommitChecks)>,
    },
    /// Corvene `787-commit-to-new-branch`: the commit form's "Commit to New
    /// Branch…" (a name for the branch the changes are committed on).
    CommitToNewBranch {
        repo: u64,
        summary: String,
        description: String,
    },
    /// Corvene `787-commit-to-new-branch`: History › "Create Branch from
    /// Commits…" for the current branch's newest commits.
    CreateBranchFromCommits {
        repo: u64,
        plan: crate::new_branch_flows::FromCommitsPlan,
    },
    /// Corvene `732-confirm-commit-to-default-branch`: committing on the
    /// default branch; "Commit" goes on to `UnknownAuthors` when
    /// `unknown_co_authors` is not empty.
    ConfirmCommitToDefaultBranch {
        repo: u64,
        branch: String,
        summary: String,
        description: String,
        unknown_co_authors: Vec<String>,
    },
    /// `ConfirmDiscardSelection`: lines picked from the diff gutter menu.
    ConfirmDiscardSelection {
        repo: u64,
        path: String,
        selection: corvene_models::DiffSelection,
    },
    /// `WarningBeforeReset`: dirty working directory before `reset --mixed`.
    ResetToCommit {
        repo: u64,
        sha: String,
    },
    /// Corvene `1216-recent-activity`: confirm `reset --hard` to a reflog
    /// entry's commit; `dirty` changed files are stashed first.
    ResetToReflogEntry {
        repo: u64,
        sha: String,
        /// The branch being reset, `None` on a detached HEAD.
        branch: Option<String>,
        dirty: usize,
    },
    /// Corvene addition (`261-reset-to-remote`): confirm resetting the
    /// current branch to its upstream (`reset --hard`), or to a commit
    /// (`888-reset-modes`' Hard reset).
    ResetToRemote {
        repo: u64,
        branch: String,
        /// Short name (`origin/main`), or the commit's short sha.
        upstream: String,
        /// Commits on the branch but not on the upstream (that the reset drops).
        ahead: usize,
        /// Uncommitted changes will be discarded too.
        dirty: bool,
        /// `888-reset-modes`: the commit a Hard reset goes to (`None`: the upstream).
        commit: Option<String>,
        /// `888-reset-modes`: the changed files whose changes are discarded.
        files: Vec<String>,
    },
    /// `ConfirmCheckoutCommit`: detached HEAD warning.
    CheckoutCommit {
        repo: u64,
        sha: String,
    },
    /// `CreateTag`
    CreateTag {
        repo: u64,
        sha: String,
    },
    /// Corvene `345-issues`: New Issue….
    NewIssue {
        repo: u64,
    },
    /// Corvene `348-pull-request-review`: Review changes… (comment,
    /// approve or request changes with the pending review's comments).
    SubmitPullRequestReview {
        repo: u64,
    },
    /// Corvene `1218-compare-refs`: Compare…, its pickers filled with
    /// `base` and `head`.
    CompareRefs {
        repo: u64,
        base: Option<String>,
        head: Option<String>,
    },
    /// Corvene `346-releases`: Create Release… for `tag` (none: a new tag)
    /// at `sha` (none: the current branch's tip).
    CreateRelease {
        repo: u64,
        tag: Option<String>,
        sha: Option<String>,
    },
    /// Corvene `527-multiple-accounts`: several accounts can push to `repo`
    /// (or see it while private); which one it uses (`logins`, the pick
    /// first, already set).
    ChooseRepositoryAccount {
        repo: u64,
        logins: Vec<String>,
    },
    /// Corvene `1212-bisect`: stash the uncommitted changes on `branch`
    /// before bisecting (`mark` as for `Dispatcher::start_bisect`).
    StartBisect {
        repo: u64,
        mark: Option<(corvene_git::BisectVerdict, String)>,
        branch: String,
    },
    /// `WarnLocalChangesBeforeUndo`
    WarnLocalChangesBeforeUndo {
        repo: u64,
    },
    /// Flag `826`: delete a tag that is not in `tagsToPush`. With
    /// `remote_only` (`1219-tag-manager`), delete it from the remote and
    /// keep it here.
    ConfirmDeletePushedTag {
        repo: u64,
        tag: String,
        remote_only: bool,
    },
    /// Flag `819`: the commit being undone carries tags.
    WarnTaggedCommitBeforeUndo {
        repo: u64,
        tags: Vec<String>,
        /// History's Undo Commit goes on to the local-changes warning;
        /// the Changes view's Undo button undoes straight away.
        warn_local: bool,
    },
    /// Flag `819`: the commit about to be amended carries tags.
    WarnTaggedCommitBeforeAmend {
        repo: u64,
        sha: String,
        tags: Vec<String>,
    },
    /// `CreateBranch`; `target_sha` when created from a commit in History.
    CreateBranch {
        repo: u64,
        target_sha: Option<String>,
        /// The branch filter text, prefilled as the name (`onCreateNewBranch`).
        initial_name: String,
    },
    RenameBranch {
        repo: u64,
        name: String,
    },
    /// GHD `ChangeRepositoryAlias` (repository list context menu).
    ChangeRepositoryAlias {
        repo: u64,
    },
    /// Corvene (`290-custom-repository-groups`): the repository list's
    /// "Move to Group…".
    MoveRepositoryToGroup {
        repo: u64,
    },
    /// Worktrees (GHD 3.6 `enableWorktreeSupport`).
    AddWorktree {
        repo: u64,
        /// `initialBranchName` / `initialWorktreeName` (checkout in a new
        /// worktree from the pull request list).
        initial_branch_name: Option<String>,
        initial_worktree_name: Option<String>,
    },
    RenameWorktree {
        repo: u64,
        path: PathBuf,
    },
    DeleteWorktree {
        repo: u64,
        path: PathBuf,
    },
    /// `git worktree remove` failed: offer `--force`; dismissing switches
    /// back to `original` when the current worktree was the one deleted.
    DeleteWorktreeFailed {
        repo: u64,
        path: PathBuf,
        error: String,
        original: Option<PathBuf>,
    },
    DeleteBranch {
        repo: u64,
        name: String,
    },
    /// Corvene (`336-request-reviewers`): ask collaborators for a review of
    /// the current branch's pull request `number`.
    RequestReviewers {
        repo: u64,
        number: u64,
    },
    /// Corvene (`895-bulk-delete-branches`): delete the branch list's
    /// multi-selection of local branches.
    DeleteBranches {
        repo: u64,
        names: Vec<String>,
    },
    /// `StashAndSwitchBranch`: ask what to do with local changes.
    StashAndSwitchBranch {
        repo: u64,
        branch: String,
    },
    /// `ConfirmOverwriteStash`: `branch` is `branchToCheckout` (`None`:
    /// Stash All Changes).
    ConfirmOverwriteStash {
        repo: u64,
        branch: Option<String>,
    },
    /// Corvene: confirm a checkout from the branch list
    /// (`864-confirm-branch-switch`).
    ConfirmSwitchBranch {
        repo: u64,
        branch: String,
    },
    /// `MultiCommitOperation` ChooseBranch step for merge (`squash` = Squash and Merge).
    MergeBranch {
        repo: u64,
        squash: bool,
    },
    /// `ConfirmDiscardStash`
    ConfirmDiscardStash {
        repo: u64,
    },
    /// Corvene (`283-move-changes-to-worktree`): pick the worktree the
    /// changes move to.
    MoveChangesToWorktree {
        repo: u64,
    },
    /// Corvene (`774-stash-conflict-flow`): every conflict a restore left is
    /// resolved; drop the entry git kept?
    DropKeptStash {
        repo: u64,
        stash: corvene_models::StashEntry,
    },
    /// Corvene (`797-stash-list`): discard this entry of the stash list?
    ConfirmDropStashEntry {
        repo: u64,
        stash: corvene_models::StashEntry,
    },
    /// Corvene (`797-stash-list`): Stash All Changes with Message.
    StashWithMessage {
        repo: u64,
    },
    /// Corvene (`1313-changelists`): New Changelist… holding `paths`, or
    /// Edit Changelist… of list `edit`.
    NewChangelist {
        repo: u64,
        paths: Vec<String>,
        edit: Option<u64>,
    },
    /// Corvene (`1105-clean-untracked-files`): the `git clean` dry run with
    /// a checkbox per path (`RepositoryState::clean_preview`).
    CleanUntrackedFiles {
        repo: u64,
    },
    /// Corvene (`1111-submodules`): the submodules and their actions
    /// (`RepositoryState::submodules`).
    Submodules {
        repo: u64,
    },
    /// Corvene (`1112-sparse-checkout`): the folders to check out
    /// (`RepositoryState::sparse_editor`).
    SparseCheckout {
        repo: u64,
    },
    /// Corvene (`1113-lfs-locks`): release a lock someone else holds.
    ConfirmForceUnlock {
        repo: u64,
        path: String,
        owner: String,
    },
    /// Corvene (`1106-apply-patch`): the files patch `name` touches, before
    /// it is applied.
    ApplyPatch {
        repo: u64,
        /// The file's name, or "Clipboard".
        name: String,
        patch: std::sync::Arc<Vec<u8>>,
        preview: corvene_git::PatchPreview,
    },
    /// Corvene (`797-stash-list`): name the branch `git stash branch` makes.
    CreateBranchFromStash {
        repo: u64,
        stash: corvene_models::StashEntry,
    },
    /// `MultiCommitOperation`: the dialog for the current `RepositoryState::mco` step.
    /// `flow` changes per operation so the dialog view is rebuilt (no stale
    /// branch selection from an earlier flow).
    MultiCommitOperation {
        repo: u64,
        flow: u64,
    },
    /// `LocalChangesOverwritten`: the operation needs a clean working directory.
    LocalChangesOverwritten {
        repo: u64,
        retry: RetryAction,
        files: Vec<String>,
    },
    /// `PushBranchCommits`: the branch must be published (`unpushed: None`)
    /// or has local commits to push before its pull request is created.
    PushBranchCommits {
        repo: u64,
        branch: String,
        unpushed: Option<u32>,
        base: Option<String>,
    },
    /// `PublishRepository`
    PublishRepository {
        repo: u64,
    },
    /// `PushNeedsPull`
    PushNeedsPull {
        repo: u64,
    },
    /// Corvene `1222-push-target-guard`: Push would send `branch` to its
    /// differently named upstream, the remote's default branch.
    ConfirmPushToUpstream {
        repo: u64,
        branch: String,
        /// `origin/main`
        upstream: String,
        /// `main`
        remote_branch: String,
        /// The held-back push's own arguments, for Push to main.
        force_with_lease: bool,
        up_to: Option<String>,
    },
    /// `ConfirmForcePush`
    ConfirmForcePush {
        repo: u64,
        upstream_branch: String,
    },
    /// `GenericGitAuthentication`
    GenericGitAuthentication {
        repo: u64,
        remote_url: String,
        host: String,
        username: Option<String>,
        retry: RetryAction,
    },
    /// Corvene (`528-proxy-credentials`): a proxy answered 407 Proxy
    /// Authentication Required.
    ProxyAuthentication {
        /// `host:port`.
        proxy: String,
        /// The username saved for it, if any.
        username: Option<String>,
        /// The saved credentials were refused.
        rejected: bool,
        retry: crate::proxy::ProxyRetry,
    },
    /// `InitializeLFS`
    InitializeLFS {
        repos: Vec<u64>,
    },
    /// `CommitMessage` popup used for the squashed commit's message.
    SquashCommitMessage {
        repo: u64,
        to_squash: Vec<String>,
        onto: String,
        summary: String,
        description: String,
        count: usize,
    },
    /// Corvene `1221-stacked-branch-refs`: a rewrite replays commits other
    /// local branches point at; move them along (`update_refs`) or not.
    WarnStackedBranches {
        repo: u64,
        branches: Vec<String>,
        update_refs: corvene_git::UpdateRefs,
        op: crate::stacked_refs::StackedOp,
    },
    /// `Preferences` (Settings…), opened on a tab.
    Preferences {
        tab: PreferencesTab,
    },
    /// Corvene › Flags… (no GHD equivalent), optionally pre-filtered.
    Flags {
        query: Option<String>,
    },
    /// Settings › Appearance › Language extensions… (no GHD equivalent;
    /// flag `111-language-extensions`). `return_to` reopens Settings on
    /// that tab when the dialog closes.
    LanguageExtensions {
        focus: Option<crate::extensions::ExtensionsFocus>,
        return_to: Option<PreferencesTab>,
    },
    /// `RepositorySettings`
    RepositorySettings {
        repo: u64,
        tab: RepositorySettingsTab,
    },
    /// `RemoveRepository` confirmation.
    ConfirmRemoveRepository {
        repo: u64,
    },
    /// `About`
    About {
        version: String,
    },
    /// `ExternalEditorError`
    ExternalEditorError {
        message: String,
        suggest_default_editor: bool,
        open_preferences: bool,
        /// Android: the editor runs in Termux, which cannot reach the
        /// repository in Corvene's own storage; "Move to shared storage…"
        /// replaces the Settings button.
        move_to_shared_storage: Option<SharedStorageMove>,
    },
    /// `OpenShellFailed`
    ShellError {
        message: String,
        /// Android: as for `ExternalEditorError`.
        move_to_shared_storage: Option<SharedStorageMove>,
    },
    /// Android (no GHD equivalent): copies a repository from Corvene's own
    /// storage to a folder on shared storage and uses it from there
    /// (`dialogs::move_to_shared_storage`).
    MoveToSharedStorage {
        repo: u64,
        then: AfterSharedStorageMove,
    },
    /// `UnreachableCommits`: which selected commits the range diff covers.
    UnreachableCommits {
        repo: u64,
        tab: UnreachableCommitsTab,
    },
    /// Corvene (`418-confirm-quit-while-busy`): Quit while a clone, push,
    /// pull, fetch or update is running; `busy` says which. It goes on the
    /// popup stack, so Cancel shows the dialog it covered again.
    ConfirmQuit {
        busy: &'static str,
    },
}

impl Popup {
    /// The repository a repository-bound dialog acts on.
    pub fn repository(&self) -> Option<u64> {
        match self {
            Self::DiscardChanges { repo, .. }
            | Self::StartPullRequest { repo, .. }
            | Self::CreateFork { repo, .. }
            | Self::UpstreamAlreadyExists { repo, .. }
            | Self::ChooseForkSettings { repo, .. }
            | Self::PushProtectionError { repo, .. }
            | Self::BypassPushProtection { repo, .. }
            | Self::PushRejectedDueToMissingWorkflowScope { repo, .. }
            | Self::SAMLReauthRequired { repo, .. }
            | Self::TestNotifications { repo, .. }
            | Self::CICheckRunRerun { repo, .. }
            | Self::PullRequestReview { repo, .. }
            | Self::PullRequestComment { repo, .. }
            | Self::PullRequestChecksFailed { repo, .. }
            | Self::UnknownAuthors { repo, .. }
            | Self::OversizedFiles { repo, .. }
            | Self::AddEmbeddedRepositories { repo, .. }
            | Self::CommitToNewBranch { repo, .. }
            | Self::CreateBranchFromCommits { repo, .. }
            | Self::ConfirmDiscardSelection { repo, .. }
            | Self::ResetToCommit { repo, .. }
            | Self::ResetToReflogEntry { repo, .. }
            | Self::CheckoutCommit { repo, .. }
            | Self::CreateTag { repo, .. }
            | Self::NewIssue { repo, .. }
            | Self::SubmitPullRequestReview { repo, .. }
            | Self::CreateRelease { repo, .. }
            | Self::ChooseRepositoryAccount { repo, .. }
            | Self::CompareRefs { repo, .. }
            | Self::StartBisect { repo, .. }
            | Self::WarnLocalChangesBeforeUndo { repo, .. }
            | Self::CreateBranch { repo, .. }
            | Self::RenameBranch { repo, .. }
            | Self::ChangeRepositoryAlias { repo, .. }
            | Self::MoveRepositoryToGroup { repo, .. }
            | Self::AddWorktree { repo, .. }
            | Self::RenameWorktree { repo, .. }
            | Self::DeleteWorktree { repo, .. }
            | Self::DeleteWorktreeFailed { repo, .. }
            | Self::DeleteBranch { repo, .. }
            | Self::DeleteBranches { repo, .. }
            | Self::RequestReviewers { repo, .. }
            | Self::StashAndSwitchBranch { repo, .. }
            | Self::ConfirmOverwriteStash { repo, .. }
            | Self::MergeBranch { repo, .. }
            | Self::ConfirmDiscardStash { repo, .. }
            | Self::DropKeptStash { repo, .. }
            | Self::ConfirmDropStashEntry { repo, .. }
            | Self::StashWithMessage { repo }
            | Self::NewChangelist { repo, .. }
            | Self::CleanUntrackedFiles { repo }
            | Self::Submodules { repo }
            | Self::SparseCheckout { repo }
            | Self::ConfirmForceUnlock { repo, .. }
            | Self::ApplyPatch { repo, .. }
            | Self::CreateBranchFromStash { repo, .. }
            | Self::MoveChangesToWorktree { repo, .. }
            | Self::MultiCommitOperation { repo, .. }
            | Self::LocalChangesOverwritten { repo, .. }
            | Self::PushBranchCommits { repo, .. }
            | Self::PublishRepository { repo, .. }
            | Self::PushNeedsPull { repo, .. }
            | Self::ConfirmForcePush { repo, .. }
            | Self::ConfirmPushToUpstream { repo, .. }
            | Self::GenericGitAuthentication { repo, .. }
            | Self::SquashCommitMessage { repo, .. }
            | Self::WarnStackedBranches { repo, .. }
            | Self::RepositorySettings { repo, .. }
            | Self::ConfirmRemoveRepository { repo, .. }
            | Self::MoveToSharedStorage { repo, .. }
            | Self::UnreachableCommits { repo, .. }
            | Self::ConfirmDeleteUntrashable { repo, .. }
            | Self::IgnoreWithPattern { repo, .. } => Some(*repo),
            Self::ActionsJobLog { repo, .. } | Self::Actions { repo, .. } => *repo,
            _ => None,
        }
    }

    /// GHD `popup.type`: which kind of popup this is (its variant).
    pub fn popup_type(&self) -> crate::popup_manager::PopupType {
        std::mem::discriminant(self)
    }

    /// A GHD `PopupType.Error` popup: errors may repeat on the popup stack
    /// and stay on top (`IndexLockExists` is flag `265`'s error dialog).
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. } | Self::IndexLockExists { .. })
    }
}

/// GHD `UnreachableCommitsTab`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UnreachableCommitsTab {
    #[default]
    Unreachable,
    Reachable,
}

/// GHD `DropTarget`: what the dragged commits currently hover, for the
/// drag element's tooltip ("Copy to <branch>", "Squash N commits", …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropTarget {
    Branch(String),
    Commit,
    /// Reorder insertion line; `count` = commits being dragged.
    InsertionPoint {
        count: usize,
    },
}

/// GHD `PreferencesTab` (Copilot omitted).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PreferencesTab {
    #[default]
    Accounts,
    Integrations,
    Git,
    Appearance,
    Notifications,
    Prompts,
    Advanced,
    Accessibility,
}

/// GHD `RepositorySettingsTab` (Fork Behavior omitted).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RepositorySettingsTab {
    #[default]
    Remote,
    IgnoredFiles,
    GitConfig,
    /// "Fork Behavior" (forks with a known parent only).
    ForkSettings,
    /// Corvene (`518-per-repo-editor`): the repository's external editor.
    Editor,
    /// Corvene (`341-custom-autolinks`): links for references like
    /// `TICKET-123` in commit messages.
    Autolinks,
}

/// GHD `GitConfigLocation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GitConfigLocation {
    #[default]
    Global,
    Local,
}

/// What the Settings › Git tab edits: read from the global git config when
/// the dialog opens (`isLoadingGitConfig` until then).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GlobalGitConfig {
    pub name: Option<String>,
    pub email: Option<String>,
    pub default_branch: String,
    /// `517-path-git-settings`: `core.quotepath` (git's default: on).
    pub quotepath: bool,
    /// `517-path-git-settings`: `core.longpaths` (Git for Windows; off).
    pub longpaths: bool,
    /// `526-commit-signing`: the global signing settings.
    pub signing: SigningConfig,
}

/// Corvene (`526-commit-signing`): commit signing in one Git config scope.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SigningConfig {
    /// `commit.gpgsign`
    pub sign: bool,
    /// `user.signingkey` (empty: unset)
    pub key: String,
    /// `gpg.format` is `ssh` (else OpenPGP, git's default)
    pub ssh: bool,
}

/// Everything the Repository Settings dialog needs, loaded when it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositorySettingsData {
    pub repo: u64,
    /// `defaultRemote` (origin, else the first remote).
    pub remote: Option<Remote>,
    /// Root `.gitignore` text, `None` when the file does not exist.
    pub gitignore: Option<String>,
    /// `--local` `user.name` / `user.email`.
    pub local_name: Option<String>,
    pub local_email: Option<String>,
    pub global: Identity,
    /// `core.autocrlf` (line endings written to `.gitignore`).
    pub autocrlf: bool,
    /// `--local` `core.autocrlf` (`239-line-endings-setting`).
    pub local_autocrlf: Option<String>,
    /// `526-commit-signing`: the signing settings in effect in the
    /// repository (its own config over the global one).
    pub local_signing: SigningConfig,
    /// `1109-remote-manager`: every remote, and where pushes go.
    pub remotes: Vec<Remote>,
    pub push_config: crate::remote_manager::PushTargetConfig,
}

/// GHD `RetryAction` (the subset behind `LocalChangesOverwritten`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryAction {
    CherryPick {
        target: String,
    },
    CherryPickNewBranch {
        name: String,
        start_point: Option<String>,
    },
    Squash {
        to_squash: Vec<String>,
        onto: String,
        message: String,
    },
    Reorder {
        to_move: Vec<String>,
        before: Option<String>,
    },
    /// Corvene `799-fixup-commits`: Squash Fixup Commits.
    Autosquash,
    Push {
        force_with_lease: bool,
        branch: Option<String>,
        /// Push only up to this commit (flag `816`).
        up_to: Option<String>,
    },
    Pull,
    Fetch,
    /// Rebase the current branch (or `target`, Corvene
    /// `1224-rebase-branch-onto-current`) onto `base` (flag `833`).
    Rebase {
        base: String,
        target: Option<String>,
    },
    /// Branch › Push To ▸ `remote` (flag `1210-push-to-other-remote`).
    PushToRemote {
        remote: String,
    },
}

impl RetryAction {
    /// `getRetryActionName`
    pub fn name(&self) -> &'static str {
        match self {
            RetryAction::CherryPick { .. } | RetryAction::CherryPickNewBranch { .. } => {
                "cherry-pick"
            }
            RetryAction::Squash { .. } | RetryAction::Autosquash => "squash",
            RetryAction::Reorder { .. } => "reorder",
            RetryAction::Push { .. } | RetryAction::PushToRemote { .. } => "push",
            RetryAction::Pull => "pull",
            RetryAction::Fetch => "fetch",
            RetryAction::Rebase { .. } => "rebase",
        }
    }
}

/// How far an authentication flow (device code, browser, personal access
/// token) is, driven by `Dispatcher::sign_in_*`: Corvene's detail of GHD's
/// Authentication step (`AppState::sign_in_store` has the step itself).
#[derive(Clone, Debug, PartialEq)]
pub enum AuthenticationStep {
    /// Asking GitHub for a device code.
    Requesting,
    /// Show the code; poll until the user authorises in the browser.
    DeviceCode {
        user_code: String,
        verification_uri: String,
    },
    /// Token in hand; fetching the account.
    Verifying,
    /// Browser (web application) flow: GitHub's authorize page is open;
    /// waiting for the callback with the code.
    Browser {
        authorize_url: String,
    },
    Error(String),
}

/// An authentication flow in progress (`AppState::authentication`).
#[derive(Clone, Debug)]
pub struct AuthenticationFlow {
    /// API base, e.g. `https://api.github.com`.
    pub endpoint: String,
    pub step: AuthenticationStep,
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// The browser flow in progress (`SignInStore.oauthState`): CSRF state
    /// and PKCE verifier the callback must match.
    pub web_flow: Option<PendingWebFlow>,
}

/// GHD `oauthState`: the browser flow waiting for its callback.
#[derive(Clone, Debug)]
pub struct PendingWebFlow {
    pub flow: corvene_github::auth::WebFlow,
    /// The loopback listener, kept alive until the flow ends.
    pub loopback: Option<Arc<corvene_github::auth::LoopbackListener>>,
}

impl PartialEq for AuthenticationFlow {
    fn eq(&self, other: &Self) -> bool {
        self.endpoint == other.endpoint && self.step == other.step
    }
}

/// GHD `CloningRepositoryID`: clone ids start high enough never to collide
/// with a repository id.
static NEXT_CLONE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_000_000);

/// An in-flight `git clone` shown in the content area (GHD
/// `CloningRepository` with its `ICloneProgress`), kept in
/// `AppState::cloning` (`crate::cloning_repositories_store`).
#[derive(Clone, Debug, PartialEq)]
pub struct CloneState {
    /// GHD `CloningRepository.id`: unique per clone, so several clones can
    /// run and be told apart.
    pub id: u64,
    pub url: String,
    pub path: PathBuf,
    pub description: String,
    /// 0..1, `None` = indeterminate.
    pub value: Option<f32>,
    /// Stops the clone (`234-clone-cancel`, `Dispatcher::cancel_clone`).
    pub cancel: corvene_git::CancelToken,
    /// Corvene (`293-clone-multiple`): this clone's place in the queue of
    /// several and the queue's length ("Cloning 2 of 5").
    pub queue: Option<(usize, usize)>,
}

impl CloneState {
    /// GHD `new CloningRepository(path, url)`: a clone with a new id that
    /// has not reported any progress yet.
    pub fn new(path: PathBuf, url: String) -> Self {
        Self {
            id: NEXT_CLONE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            url,
            path,
            description: String::new(),
            value: None,
            cancel: corvene_git::CancelToken::new(),
            queue: None,
        }
    }

    /// GHD `CloningRepository.name` (`Path.basename(url, '.git')`): the
    /// repository name the cloning view shows ("Cloning desktop").
    pub fn name(&self) -> String {
        basename_without(&self.url, ".git")
    }
}

/// Node's `path.basename(path, ext)`: the last component (trailing
/// separators ignored) without `ext` when it ends with it and is more than
/// it.
fn basename_without(path: &str, ext: &str) -> String {
    let separators: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    let trimmed = path.trim_end_matches(separators);
    let base = trimmed
        .rfind(separators)
        .map_or(trimmed, |ix| &trimmed[ix + 1..]);
    match base.strip_suffix(ext) {
        Some(stem) if !stem.is_empty() => stem.to_string(),
        _ => base.to_string(),
    }
}

/// Android: a repository in Corvene's own storage that Termux could not
/// reach, and what runs again once it was moved to shared storage
/// (`Popup::MoveToSharedStorage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedStorageMove {
    pub repo: u64,
    pub then: AfterSharedStorageMove,
}

/// What `Dispatcher::move_to_shared_storage` runs at the new location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterSharedStorageMove {
    Nothing,
    /// "Open in Termux" at the repository root.
    OpenShell,
    /// A Termux editor on `relative` (to the repository root), at `line`.
    OpenEditor {
        relative: PathBuf,
        line: Option<u32>,
    },
}

/// A move to shared storage in progress (`AppState::shared_storage_move`).
#[derive(Clone, Debug, PartialEq)]
pub struct SharedStorageMoveState {
    pub repo: u64,
    pub destination: PathBuf,
    pub stage: SharedStorageMoveStage,
    /// Stops the copy; the partial copy is removed.
    pub cancel: corvene_git::CancelToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedStorageMoveStage {
    /// `done` of `total` files and folders copied.
    Copying { done: u64, total: u64 },
    /// git is reading the copy (configuration, index refresh, open).
    Checking,
    /// The move stopped; the dialog shows why and offers the form again.
    Failed(String),
}

impl SharedStorageMoveStage {
    /// No more progress will come.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// What deleting a branch would lose (`860-delete-branch-warnings`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteBranchPreview {
    pub branch: String,
    /// Commits on the branch that neither the default branch (local or its
    /// upstream) nor the branch's own upstream contain.
    pub unmerged_commits: u32,
    /// What those commits were compared against, for the message.
    pub compared_to: Vec<String>,
    /// A GitHub Desktop / Corvene stash entry is recorded for the branch.
    pub has_stash: bool,
}

/// How many incoming commits [`RepositoryState::incoming_commits`] keeps.
/// `708-changes-busy-indicator`: how long a refresh runs before the Changes
/// list shows a spinner.
pub const BUSY_INDICATOR_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

pub const INCOMING_COMMITS_LIMIT: usize = 10;

/// `291-recent-worktrees`: how many (repository, worktree) uses are kept.
pub const RECENT_WORKTREES_LENGTH: usize = 30;

/// Per-repository cache (`IRepositoryState`, trimmed).
#[derive(Clone, Debug, Default)]
pub struct RepositoryState {
    pub info: Option<RepositoryInfo>,
    pub ahead_behind: Option<AheadBehind>,
    pub loading: bool,
    pub error: Option<String>,
    /// GHD `RepositoryType` `unsafe`: the directory git named as having
    /// dubious ownership ("Trust Repository" view).
    pub unsafe_path: Option<PathBuf>,
    /// `isTrustingPath`: `safe.directory` is being added.
    pub trusting_path: bool,
    pub last_refresh: Option<Instant>,
    pub section: Section,
    /// `git status` result (`IChangesState.workingDirectory`). Shared: a
    /// refresh hands it to the next one and views key caches on the
    /// pointer, so every change goes through `Arc::make_mut`.
    pub status: Option<Arc<WorkingDirectoryStatus>>,
    /// Lines added / deleted per changed file against HEAD (Corvene
    /// addition, flag `changes-line-counts`; empty while the flag is off).
    pub line_stats: Arc<HashMap<String, corvene_git::LineStats>>,
    /// Path of the file whose diff is shown (`selectedFileIDs[0]`).
    pub selected_file: Option<String>,
    /// Every selected path (`selectedFileIDs`), click order; ⌘/⇧-click extend it.
    pub selected_files: Vec<String>,
    pub diff: Option<Arc<Diff>>,
    /// `790-selection-follows-lines`: for each file with a partial line
    /// selection, the diff that selection names lines of.
    pub selection_bases: HashMap<String, Arc<Diff>>,
    /// `1208-undo-restores-line-selection`: the line selections an undone
    /// commit had made, put on each file when a status lists it.
    pub restored_selections: HashMap<String, corvene_models::DiffSelection>,
    /// `793-hide-whitespace-only-files`: the changed files whose changes
    /// are whitespace only, while Changes hides whitespace.
    pub whitespace_only_files: Option<Arc<std::collections::HashSet<String>>>,
    pub diff_loading: bool,
    /// `749-binary-diff-as-text`: the path whose diff was asked for with
    /// `--text` ("Show diff anyway" on a binary file).
    pub diff_as_text: Option<String>,
    /// Bumped whenever `diff` is replaced, so views can cache derived rows.
    pub diff_generation: u64,
    /// The new side of the selected file as lines, for hunk expansion
    /// (GHD `fileContents.newContents`); `None` when it cannot be expanded.
    pub diff_contents: Option<Arc<Vec<String>>>,
    /// The old side (`HEAD:<old path>`), for syntax highlighting like GHD
    /// (`fileContents.oldContents`); `None` for new files.
    pub diff_old_contents: Option<Arc<Vec<String>>>,
    /// `762-too-large-diff-escape-hatch`: the repository's `diff.tool`, read
    /// when a diff turned out too large to show.
    pub diff_tool: Option<String>,
    /// `763-diff-header-mtime`: the selected working-directory file's path
    /// and modification time.
    pub diff_file_modified: Option<(String, std::time::SystemTime)>,
    /// `764-cancel-stale-diffs`: stops the working-directory diff still
    /// being computed when another one is asked for.
    pub diff_cancel: Option<corvene_git::CancelToken>,
    /// Most recent commit made from Corvene in this session (`UndoCommit` bar).
    pub last_commit: Option<LastCommit>,
    /// Summaries of the upstream's commits the current branch lacks
    /// (`HEAD..upstream`, newest first, at most [`INCOMING_COMMITS_LIMIT`]),
    /// for the Pull button's tooltip (flag `257`).
    pub incoming_commits: Vec<String>,
    /// `883-unpublished-commit-links`: HEAD's commits no remote-tracking
    /// branch contains (`None`: not known, or the flag is off).
    pub unpublished_commits: Option<std::collections::HashSet<String>>,
    /// GHD `localCommitSHAs`: the current branch's commits no remote has
    /// (`upstream..branch`, else `HEAD --not --remotes`), for the history
    /// rows' unpushed indicator. Paged alongside the commit list, so its
    /// length is the next page's `skip`.
    pub local_commits: std::collections::HashSet<String>,
    /// Incremented after every successful commit so the form can clear itself.
    pub commit_nonce: u64,
    /// `723-discard-confirm-snooze`: discarding (not all changes) skips the
    /// confirmation until then (this session only).
    pub discard_confirm_snoozed_until: Option<Instant>,
    /// GHD `showCoAuthoredBy` / `coAuthors` (per repository, this session).
    pub show_co_authored_by: bool,
    pub co_authors: Vec<corvene_models::Author>,
    pub committing: bool,
    /// GHD `hookProgress`: the commit's hook running or just done (hooks
    /// interception, Settings › Git › Hooks).
    pub hook_progress: Option<corvene_git::hooks::HookProgress>,
    /// GHD `subscribeToCommitOutput`: the output of the commit in progress.
    pub commit_output: Option<crate::hooks::CommitOutput>,
    /// Corvene `1114-hook-results`: the last operation's non-blocking hooks
    /// (the sidebar's hook list) until dismissed.
    pub hook_report: Option<crate::hooks::HookReport>,
    /// Corvene `1222-push-target-guard`: the upstream the next push may go
    /// to without asking (Confirm Push › Push to main).
    pub push_target_confirmed: Option<String>,
    /// Corvene `1307-commit-progress`: how far the commit in progress got,
    /// once it has run for a moment.
    pub commit_progress: Option<crate::commit_progress::CommitProgress>,
    /// Discard Changes is running (`708-changes-busy-indicator`).
    pub discarding: bool,
    /// A refresh was requested while one was running; run again when done.
    pub refresh_pending: bool,
    /// When the running refresh started.
    pub refresh_started: Option<Instant>,
    /// GHD `clearPartialState`: the next status a refresh reads drops
    /// partial line selections (set after a commit and when Hide Whitespace
    /// changes in Changes; `crate::changes_state::apply_changed_files`).
    pub clear_partial_state: bool,
    /// The branch pruner waits for the refresh in progress (it reads the
    /// default branch and the branches the refresh loads).
    pub prune_after_refresh: bool,
    /// Filter Options popover state (`IFileListFilterState` minus the text).
    pub file_list_filter: FileListFilter,

    // ---- history (`ICompareState` / `ICommitSelection`) ----
    /// Commits of HEAD, newest first, loaded in `COMMIT_BATCH_SIZE` pages.
    pub commits: Vec<corvene_models::Commit>,
    pub commits_loading: bool,
    /// `885-history-load-race`: a first-page reload was asked for while a
    /// page was loading; it runs when that one is in.
    pub commits_reload_pending: bool,
    /// The last page was shorter than a batch: nothing more to load.
    pub commits_exhausted: bool,
    /// `1213-commit-graph`: every branch's commits while History shows
    /// All branches (`None` otherwise).
    pub all_branches: Option<crate::commit_graph::AllBranchesHistory>,
    /// `commitSelection.shas[0]`: the anchor of the selection.
    pub selected_commit: Option<String>,
    /// `commitSelection.shas` in click order (⌘/⇧-click multi-select).
    pub selected_commits: Vec<String>,
    /// `commitSelection.isContiguous`
    pub commits_contiguous: bool,
    /// `commitSelection.shasInDiff`: the selected commits reachable from the newest one.
    pub shas_in_diff: Vec<String>,
    /// Files + line counts of the selected commit (`changesetData`).
    pub changeset: Option<corvene_models::ChangesetData>,
    /// `793-hide-whitespace-only-files`: how many of the commit's files
    /// `changeset` leaves out because their changes are whitespace only.
    pub changeset_whitespace_hidden: usize,
    /// Path selected in the commit's file list.
    pub commit_selected_file: Option<String>,
    pub commit_diff: Option<Arc<Diff>>,
    pub commit_diff_generation: u64,
    pub commit_diff_contents: Option<Arc<Vec<String>>>,
    /// `<oldest selected>^:<old path>` (`parentCommitish`), for highlighting.
    pub commit_diff_old_contents: Option<Arc<Vec<String>>>,
    /// `isExpanded` of the expandable commit summary.
    pub commit_summary_expanded: bool,
    /// `773-merge-remerge-diff`: a selected merge shows only its conflict
    /// resolutions (this session).
    pub remerge_diff: bool,
    /// `commitToAmend`: the commit form rewrites HEAD instead of adding a commit.
    pub commit_to_amend: Option<corvene_models::Commit>,
    /// Bumped when amending starts so the form loads the commit's message.
    pub amend_nonce: u64,
    /// Corvene `783-amend-author`: the author the amended commit gets, when
    /// the commit form's author field changed it.
    pub amend_author: Option<corvene_git::CommitAuthor>,
    /// GHD `IChangesState.commitMessage`: the message the dispatcher hands
    /// to the commit form (Undo Commit's, `git_store::undo_commit`); the
    /// form loads it when `commit_message_nonce` changes. Corvene's form
    /// keeps what is typed itself, so this does not follow the typing.
    pub commit_message: CommitMessage,
    pub commit_message_nonce: u64,

    // ---- branches (`IBranchesState`) ----
    /// `recentBranches` (reflog checkouts, newest first).
    pub recent_branches: Vec<String>,
    /// `git worktree list` (main first); the toolbar button shows once
    /// there is more than one.
    pub worktrees: Vec<corvene_models::WorktreeEntry>,
    /// `defaultBranch` name (`findDefaultBranch`).
    pub default_branch: Option<String>,
    /// Corvene `1221-stacked-branch-refs`: History's marks for the commits
    /// the default branch does not have (read with the refresh).
    pub stacked_refs: Option<std::sync::Arc<crate::stacked_refs::StackedRefs>>,
    /// Branch a checkout is switching to (`checkoutProgress.target`).
    pub checkout_target: Option<String>,
    /// Corvene/GHD stash entry for the current branch (`changesState.stashEntry`);
    /// with `728-show-latest-other-stash`, else the newest stash no Desktop made.
    pub stash: Option<corvene_models::StashEntry>,
    /// Local branches' upstream state by name, read while
    /// `852-branch-upstream-gone` or `853-branch-list-ahead-behind` is on.
    pub branch_tracking: Arc<std::collections::HashMap<String, corvene_git::BranchTracking>>,
    /// Total stash entries (`stashEntryCount`).
    pub stash_count: usize,
    /// `1207-switch-warns-target-behind`: (branch, its upstream, how many
    /// of the upstream's commits the branch lacks) for the Switch Branch
    /// dialog, read when it opens.
    pub switch_target_behind: Option<(String, String, u32)>,
    /// `774-stash-conflict-flow`: the entry git kept after a restore that
    /// conflicted (this session).
    pub kept_stash: Option<crate::stash_flows::KeptStash>,
    /// Branches with a GitHub Desktop / Corvene stash (the branch list's
    /// stash icon, `854-branch-list-stash-icon`).
    pub stashed_branches: Vec<String>,
    /// `797-stash-list`: every stash entry, newest first (empty while the
    /// flag is off).
    pub stashes: Vec<corvene_models::StashEntry>,
    /// Merge dialog preview.
    pub merge_preview: Option<crate::mco::MergePreview>,
    /// Delete Branch dialog warnings (`860-delete-branch-warnings`).
    pub delete_branch_preview: Option<DeleteBranchPreview>,
    /// `336-request-reviewers`: the repository's collaborators (logins,
    /// sorted), fetched once per session, and whether that fetch runs.
    pub collaborators: Option<Arc<Vec<String>>>,
    pub collaborators_loading: bool,
    /// `1202-update-from-parent-branch`: the branch the current branch was
    /// created from (`branch.<name>.vscode-merge-base`), when it exists.
    pub update_parent: Option<String>,
    /// `899-tags-in-branch-list`: every tag (name, commit), read when the
    /// branch list opens.
    pub branch_list_tags: Option<Arc<Vec<(String, String)>>>,
    /// Delete Branches confirmation marks (`895-bulk-delete-branches`).
    pub delete_branches_preview: Option<crate::delete_branches::DeleteBranchesPreview>,
    /// `pullRequestState`: the Preview Pull Request dialog's data.
    pub pull_request_preview: Option<crate::pull_request_preview::PullRequestPreview>,
    /// `addUpstreamRemoteIfNeeded` ran for this repository this session.
    pub upstream_checked: bool,
    /// `changesState.currentBranchProtected`
    pub current_branch_protected: bool,
    /// Corvene `339-protected-branch-bypass-note`: the branch is protected on
    /// GitHub but takes the user's direct pushes (an admin or bypass-list
    /// exception, or a pull request rule without required approvals).
    pub current_branch_protection_bypassed: bool,
    /// `changesState.currentRepoRulesInfo`
    pub repo_rules: corvene_models::RepoRulesInfo,
    /// Which branch the rules were fetched for, and when.
    pub repo_rules_branch: Option<String>,
    pub repo_rules_fetched_at: Option<Instant>,
    /// GHD `checkBranchNameRules` for the name typed in Create Branch or
    /// Rename Branch (the dialogs show it while their name matches).
    pub branch_name_rule_error: Option<crate::repo_rules::BranchNameRuleError>,
    /// The name the last of those checks asked about.
    pub branch_name_rule_requested: String,
    /// `353-fork-tracks-upstream`: the `upstream` remote was just added to
    /// a fork contributing to its parent; the next refresh sets up its
    /// default branch.
    pub track_parent_after_refresh: bool,
    /// Rebase dialog preview.
    pub rebase_preview: Option<crate::mco::RebasePreview>,

    /// `compareState`
    pub compare: crate::compare::CompareState,
    /// `886-history-search`: the filter box above History.
    pub history_filter: crate::history_filter::HistoryFilter,

    // ---- multi-commit operations ----
    pub mco: Option<crate::mco::MultiCommitOperation>,
    /// Bumped by every new multi-commit operation (see `Popup::MultiCommitOperation`).
    pub mco_flow: u64,
    /// `shasToHighlight`: rows kept opaque while hovering the multi-commit
    /// summary's counts; everything else dims.
    pub highlighted_shas: Vec<String>,
    pub mco_undo: Option<crate::mco::McoUndo>,
    /// Flag `828`: the message of a squash that failed, keyed by its commits
    /// (onto, then the squashed ones), offered again by the next squash of them.
    pub squash_draft: Option<(Vec<String>, String)>,
    /// Flag `830`: commits (summary, author time) a squash / reorder just
    /// rewrote; the next history load selects their new shas.
    pub rewritten_selection: Vec<(String, Option<i64>)>,
    /// `changesState.conflictState`
    pub conflict_state: Option<crate::mco::ConflictState>,
    /// `1212-bisect`: a mark was just made; the next History load selects
    /// the first bad commit if git named it (`Dispatcher::bisect_mark`).
    pub bisect_reveal: bool,
    /// `770-co-authors-from-history`: the distinct authors (name, email) of
    /// the newest commits, read once per session; whether that read runs.
    pub recent_authors: Option<std::sync::Arc<Vec<(String, String)>>>,
    pub recent_authors_loading: bool,
    /// `forcePushBranches`: branch → tip after a rewrite that needs a force push.
    pub force_push_branches: HashMap<String, String>,
    /// The current branch is ahead of and behind its upstream, and its
    /// reflog holds the upstream's tip: commits pushed from here were
    /// rewritten outside Corvene (`260-force-push-after-outside-rewrite`).
    pub upstream_rewritten: bool,

    // ---- remote (`isPushPullFetchInProgress`, `pushPullFetchProgress`, `lastFetched`) ----
    pub push_pull_in_progress: bool,
    /// The running network operation is a background fetch that shows no
    /// progress (`245-push-during-background-fetch`).
    pub quiet_background_fetch: bool,
    pub push_pull_progress: Option<crate::remote::PushPullProgress>,
    /// Corvene (`295-cancel-network-operations`, `296-cancel-fetch-on-wake`):
    /// stops the running fetch, pull or push.
    pub network_cancel: Option<crate::remote::NetworkCancel>,
    /// When the push/pull button's Stop was used, for its "Cancelled" note.
    pub network_cancelled_at: Option<Instant>,
    /// Corvene `1101-push-size-tooltip`: the push button's size note.
    pub push_size: Option<crate::remote::PushSizeEstimate>,
    pub last_fetched: Option<std::time::SystemTime>,
    /// Corvene (`288-dead-remote-indicator`): the last fetch said the remote
    /// repository does not exist (deleted, renamed or no access); the
    /// repository list marks the row and background fetches skip it until a
    /// fetch succeeds.
    pub remote_not_found: bool,
    /// Corvene `341-custom-autolinks`: the GitHub repository's autolinks
    /// as its API gave them (only admins may read them).
    pub api_autolinks: Vec<corvene_models::Autolink>,
    /// Corvene `526-commit-signing`: the effective `commit.gpgsign` (git
    /// signs the commits made here).
    pub signs_commits: bool,
    /// Corvene `1211-issuetracker-links`: the repository's `.issuetracker`
    /// trackers, compiled when it is selected.
    pub issue_trackers: Vec<crate::text_tokens::LinkRule>,
    pub pull_with_rebase: bool,
    /// Corvene `340-message-rules-defer-to-hooks`: git runs a
    /// `prepare-commit-msg` or `commit-msg` hook on a commit (read while the
    /// flag is on).
    pub commit_message_hook: bool,
    /// Corvene `1103-implicit-upstream-push-default`: the current branch has
    /// no upstream, but `push.default=current` pushes it to this
    /// remote-tracking branch (`origin/feature`); its ahead/behind counts.
    pub implicit_upstream: Option<(String, AheadBehind)>,
    /// Corvene `1109-remote-manager`: the current branch's push remote
    /// (`branch.<name>.pushRemote` or `remote.pushDefault`) while it is not
    /// the upstream's remote.
    pub push_target: Option<crate::remote_manager::PushTarget>,
    pub publishing: bool,
    /// The LFS initialisation prompt was already considered for this repository.
    pub lfs_checked: bool,

    // ---- stash viewer (`isShowingStashEntry`, `selectedStashedFile`) ----
    pub showing_stash: bool,
    /// `797-stash-list`: the entry of [`Self::stashes`] the viewer shows
    /// instead of [`Self::stash`] (see [`Self::shown_stash`]).
    pub viewed_stash: Option<String>,
    pub stash_files: Option<Vec<corvene_models::CommittedFileChange>>,
    /// Corvene `1319-trivial-change-icon`: `stash_files`' trivial changes.
    pub stash_trivial: std::collections::BTreeMap<String, corvene_models::TrivialChange>,
    /// The stash whose files `stash_files` holds or is loading
    /// (`stashEntry.files` Loading / Loaded).
    pub stash_files_sha: Option<String>,
    pub stash_selected_file: Option<String>,
    pub stash_diff: Option<Arc<Diff>>,
    pub stash_diff_generation: u64,
    pub stash_diff_contents: Option<Arc<Vec<String>>>,
    /// `<stash>^:<old path>`, for highlighting.
    pub stash_diff_old_contents: Option<Arc<Vec<String>>>,

    // ---- `798-blame` ----
    /// The open Blame view, shown in place of the diff.
    pub blame: Option<crate::blame::BlameState>,
    /// The options the next blame runs with (kept per session).
    pub blame_options: corvene_git::BlameOptions,
    /// Bumped with the commit History should scroll to (a blamed commit
    /// selected from the gutter).
    pub reveal_commit: (u64, Option<String>),

    /// `1214-commit-signatures`: verified signatures by commit.
    pub signatures: crate::signatures::SignatureStore,

    // ---- `1311-stacked-diff` ----
    /// The diffs of the files shown stacked in one list.
    pub stacked: crate::stacked_diff::StackedDiffs,

    // ---- `1323-archive-diff` ----
    /// The archive views and the file lists' archive rows.
    pub archives: crate::archive_browse::ArchiveBrowses,

    // ---- `1218-compare-refs` ----
    /// The combined diff of the two compared refs.
    pub ref_compare_changes: Option<crate::pull_request_preview::PullRequestPreview>,

    // ---- `1110-repository-insights` ----
    /// Repository › Insights…, shown in place of the tab's content.
    pub insights: Option<crate::insights::InsightsState>,
    /// Finished statistics by scope, range and tips (newest last).
    pub insights_cache: Vec<(crate::insights::InsightsKey, Arc<corvene_git::RepoStats>)>,

    // ---- `1216-recent-activity` ----
    /// Repository › Recent Activity…: History lists the reflog instead.
    pub reflog: Option<crate::reflog::ReflogState>,
    /// Corvene `1219-tag-manager`: Branch › Tags…, History lists the tags
    /// instead.
    pub tags_view: Option<crate::tag_manager::TagsViewState>,
    /// Corvene `1223-checkout-from-fork`: Branch › Check Out from Fork….
    pub fork_checkout: Option<crate::fork_checkout::ForkCheckoutState>,

    /// `1105-clean-untracked-files`: the dry run Clean Untracked Files shows.
    pub clean_preview: Option<crate::clean_untracked::CleanPreview>,
    /// `1111-submodules`: what Repository › Submodules… lists.
    pub submodules: Option<crate::submodules::SubmodulesState>,
    /// `1112-sparse-checkout`: set while sparse checkout is on (refresh).
    pub sparse_checkout: Option<crate::sparse_checkout::SparseSummary>,
    /// `1112-sparse-checkout`: the folders Sparse Checkout… shows.
    pub sparse_editor: Option<crate::sparse_checkout::SparseEditor>,
    /// `1113-lfs-locks`: `.gitattributes` has `filter=lfs` (refresh).
    pub uses_lfs: bool,
    /// `1113-lfs-locks`: the LFS server's locks, once asked.
    pub lfs_locks: Option<crate::lfs_locks::LfsLockState>,
    /// `1314-code-owners`: the CODEOWNERS files the file lists follow.
    pub code_owners: crate::codeowners::CodeOwnersState,
    // ---- `345-issues` ----
    /// Repository › Issues…: History lists the issues instead (also holds
    /// the New Issue… labels and assignees while the list is closed).
    pub issues: Option<crate::issues::IssuesViewState>,
    /// The issue the Create Branch dialog was opened for.
    pub pending_issue_link: Option<crate::issues::PendingIssueLink>,

    // ---- `348-pull-request-review` ----
    /// Review Pull Request: History shows the pull request's files and
    /// overview instead.
    pub pull_request_review: Option<crate::pull_request_review::PullRequestReviewState>,

    // ---- `346-releases` ----
    /// Repository › Releases…: History lists the releases instead.
    pub releases: Option<crate::releases::ReleasesViewState>,
    /// Create Release › Generate release notes, in flight / its result.
    pub generating_release_notes: bool,
    pub generated_release_notes: Option<Result<crate::releases::GeneratedReleaseNotes, String>>,
}

/// GHD `IFileListFilterState` option flags; the text lives in the text box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileListFilter {
    pub included: bool,
    pub excluded: bool,
    pub new_files: bool,
    pub modified: bool,
    pub deleted: bool,
    /// Corvene `705-renamed-files-filter`.
    pub renamed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOption {
    IncludedInCommit,
    ExcludedFromCommit,
    NewFiles,
    ModifiedFiles,
    DeletedFiles,
    /// Corvene `705-renamed-files-filter`.
    RenamedFiles,
}

impl FileListFilter {
    pub fn get(&self, option: FilterOption) -> bool {
        match option {
            FilterOption::IncludedInCommit => self.included,
            FilterOption::ExcludedFromCommit => self.excluded,
            FilterOption::NewFiles => self.new_files,
            FilterOption::ModifiedFiles => self.modified,
            FilterOption::DeletedFiles => self.deleted,
            FilterOption::RenamedFiles => self.renamed,
        }
    }

    pub fn set(&mut self, option: FilterOption, on: bool) {
        match option {
            FilterOption::IncludedInCommit => self.included = on,
            FilterOption::ExcludedFromCommit => self.excluded = on,
            FilterOption::NewFiles => self.new_files = on,
            FilterOption::ModifiedFiles => self.modified = on,
            FilterOption::DeletedFiles => self.deleted = on,
            FilterOption::RenamedFiles => self.renamed = on,
        }
    }

    /// `countActiveFilterOptions`
    pub fn count_active(&self) -> usize {
        [
            self.included,
            self.excluded,
            self.new_files,
            self.modified,
            self.deleted,
            self.renamed,
        ]
        .iter()
        .filter(|b| **b)
        .count()
    }
}

/// GHD `ICommitMessage`: a commit message for the commit form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitMessage {
    pub summary: String,
    pub description: Option<String>,
}

impl CommitMessage {
    pub fn new(summary: impl Into<String>, description: Option<String>) -> Self {
        Self {
            summary: summary.into(),
            description,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastCommit {
    pub sha: String,
    pub summary: String,
    pub at: std::time::SystemTime,
}

impl RepositoryState {
    /// The commit list the History tab shows: the comparison while comparing
    /// to a branch, the filter's matches while History is filtered
    /// (`886-history-search`), else the branch's own history.
    pub fn visible_commits(&self) -> &Vec<corvene_models::Commit> {
        if let Some(reflog) = &self.reflog {
            // `1216-recent-activity`: the commits its entries point at
            &reflog.commits
        } else if let Some(tags) = &self.tags_view {
            // `1219-tag-manager`: the tagged commits
            &tags.commits
        } else if self.compare.is_comparing() {
            &self.compare.commits
        } else if self.history_filter.is_active() {
            &self.history_filter.commits
        } else if self.showing_all_branches() {
            self.all_branches
                .as_ref()
                .map_or(&self.commits, |a| &a.commits)
        } else {
            &self.commits
        }
    }

    /// The commit `sha` in the listed commits, else in HEAD's history
    /// (`1216-recent-activity` lists commits no branch has).
    pub fn find_commit(&self, sha: &str) -> Option<&corvene_models::Commit> {
        self.visible_commits()
            .iter()
            .find(|c| c.sha == sha)
            .or_else(|| self.commits.iter().find(|c| c.sha == sha))
    }

    /// History lists every branch (`1213-commit-graph`), not only the
    /// commits of HEAD: rows have no neighbours to squash or reorder with.
    /// A bisect's range (`1212-bisect`) comes first.
    pub fn showing_all_branches(&self) -> bool {
        self.reflog.is_none()
            && self.tags_view.is_none()
            && !self.compare.is_comparing()
            && !self.history_filter.is_active()
            && !self.status.as_ref().is_some_and(|s| s.bisect.is_some())
            && self.all_branches.as_ref().is_some_and(|a| a.loaded)
    }

    pub fn changed_files(&self) -> usize {
        self.status.as_deref().map(|s| s.files.len()).unwrap_or(0)
    }

    /// [`Self::stash`] when a Desktop made it for this branch: the entry a new
    /// stash replaces (a `git stash` shown by `728-show-latest-other-stash`
    /// is never dropped to make room).
    pub fn desktop_stash(&self) -> Option<&corvene_models::StashEntry> {
        self.stash.as_ref().filter(|s| s.branch.is_some())
    }

    /// The stash the stash viewer shows: the entry picked in the stash list
    /// (`797-stash-list`), else [`Self::stash`].
    /// Back from the stash viewer to the working directory's diff (GHD
    /// `_selectWorkingDirectoryFiles` replaces a `ChangesSelectionKind.Stash`
    /// selection). `797-stash-list`: the entry picked in the list is
    /// forgotten, so showing again shows the branch's own. Whether the
    /// stash was shown.
    pub fn leave_stash(&mut self) -> bool {
        if self.viewed_stash.take().is_some() {
            self.stash_files = None;
            self.stash_files_sha = None;
            self.stash_diff = None;
        }
        std::mem::take(&mut self.showing_stash)
    }

    pub fn shown_stash(&self) -> Option<&corvene_models::StashEntry> {
        self.viewed_stash
            .as_ref()
            .and_then(|sha| self.stashes.iter().find(|s| &s.sha == sha))
            .or(self.stash.as_ref())
    }
}

pub struct AppState {
    pub store: Arc<Store>,
    pub settings: Settings,
    /// Feature flags (`crate::flags`): the stored layer, this session's
    /// `CORVENE_FLAGS`, the resolved snapshot views read, and the snapshot
    /// at launch (restart-required flags compare against it).
    pub flag_overrides: crate::flags::FlagOverrides,
    pub flags_env: crate::flags::EnvFlags,
    pub flags: crate::flags::Flags,
    pub flags_at_launch: crate::flags::Flags,
    pub git: Option<Arc<GitBinary>>,
    pub git_error: Option<String>,
    /// `Dispatcher::replace_stale_git` is looking for git again.
    pub git_redetecting: bool,
    pub repositories: Vec<Repository>,
    /// Most recent first, max 3 (GHD `RecentRepositoriesLength`).
    pub recent: Vec<u64>,
    /// Corvene (`291-recent-worktrees`): the worktrees each repository was
    /// last used in, most recent first (`(id, worktree path)`, at most
    /// [`RECENT_WORKTREES_LENGTH`]); the Recent group lists a repository
    /// once per worktree.
    pub recent_worktrees: Vec<(u64, PathBuf)>,
    /// One per window (`429-multiple-windows`; `crate::workspace`): the
    /// selected repository, foldout, popups, banner and navigation history
    /// of each. Never empty: the last workspace is never removed.
    /// `AppState` derefs to the one `current` names.
    pub workspaces: Vec<WorkspaceState>,
    /// The workspace code is running for right now (see `crate::workspace`).
    pub current: WorkspaceId,
    /// The workspace of the window that was last active.
    pub focused: WorkspaceId,
    pub next_workspace_id: u32,
    /// Corvene (`618-keymap-overrides`): `keymap.json` as last read, and
    /// the problems reading it found (shown once in a banner).
    pub keymap_overrides: crate::keymap_file::KeymapOverrides,
    pub keymap_load_errors: Vec<String>,
    /// Corvene (`522-settings-file`): the keys the settings file set (saved
    /// with their stored values) and the flags it decided.
    pub settings_overlay: crate::settings_file::SettingsOverlay,
    pub settings_file_flags: crate::flags::EnvFlags,
    pub repo_states: HashMap<u64, RepositoryState>,
    pub accounts: Vec<Account>,
    /// GHD `CloningRepositoriesStore`: the clones in progress.
    pub cloning: crate::cloning_repositories_store::CloningRepositoriesStore,
    /// GHD `AheadBehindStore`: ahead/behind counts of commit ranges, cached
    /// by repository and tip shas (the compare branch list).
    pub ahead_behind: crate::ahead_behind_store::AheadBehindStore,
    /// Android: a repository being moved to shared storage.
    pub shared_storage_move: Option<SharedStorageMoveState>,
    /// `224-alias-when-adding`: aliases typed in New / Add / Clone, applied
    /// when the repository at that (resolved) path is added.
    pub pending_aliases: Vec<(PathBuf, String)>,
    /// `527-multiple-accounts`: the account logins repositories cloned or
    /// added at these (resolved) paths use (`Dispatcher::account_when_added`).
    pub pending_accounts: Vec<(PathBuf, String)>,
    /// `527-multiple-accounts`: repositories whose account was looked up.
    pub account_probes: crate::repository_accounts::AccountProbes,
    /// GHD `SignInStore`: the sign-in dialog's step.
    pub sign_in_store: crate::sign_in::SignInStore,
    /// The accounts `sign_in_store` reads (`AppState::accounts`, copied in
    /// by the dispatcher before each sign-in step).
    pub sign_in_accounts: std::rc::Rc<std::cell::RefCell<Vec<Account>>>,
    /// The authentication flow in progress.
    pub authentication: Option<AuthenticationFlow>,
    /// Corvene (`527-multiple-accounts`): the sign-in dialog was opened by
    /// Add account (`Dispatcher::show_add_account_dialog`).
    pub adding_account: bool,
    /// Corvene `350-ssh-key-helper`: scopes the next sign-in asks for
    /// besides `corvene_github::SCOPES` (cleared when the dialog closes).
    pub extra_oauth_scopes: Vec<String>,
    /// Corvene `350-ssh-key-helper`: Settings › Integrations' SSH key work.
    pub ssh_key: crate::ssh_keys::SshKeyState,
    /// Watchers of the visible repositories' worktrees (one per repository
    /// some workspace shows), each with the generation it was started at.
    pub watchers: HashMap<u64, (u64, crate::watcher::RepoWatcher)>,
    pub watcher_generation: u64,
    /// Bumped per repository by `Dispatcher::start_background_pruner`, so an
    /// earlier pruning timer of the same repository stops (GHD
    /// `currentBranchPruner`).
    pub pruner_generations: HashMap<u64, u64>,
    /// Sidebar indicators per repository (`localRepositoryStateLookup`).
    pub indicators: HashMap<u64, crate::remote::RepoIndicator>,
    /// Generic git server logins (host → username) for the askpass helper.
    pub generic_logins: HashMap<String, String>,
    /// `528-proxy-credentials`: saved proxy usernames, cancelled prompts,
    /// Settings › Advanced's proxy line.
    pub proxy: crate::proxy::ProxyState,
    /// OAuth app client IDs entered per GitHub Enterprise host (host →
    /// client ID), `Dispatcher::set_enterprise_oauth_app`.
    pub enterprise_oauth_apps: HashMap<String, String>,
    /// Avatar cache (`crate::avatars`).
    pub avatars: crate::avatars::Avatars,
    /// Images in Markdown by URL (`crate::markdown_images`).
    pub markdown_images: crate::markdown_images::MarkdownImages,
    /// Clone dialog: `GET /user/repos` per account (`ApiRepositoriesStore`),
    /// keyed by `Account::key` (`endpoint|login`).
    pub api_repositories: HashMap<String, Vec<corvene_models::GitHubRepository>>,
    pub api_repositories_loading: std::collections::HashSet<String>,
    /// `#issue` / `@user` autocompletion caches (`IssuesStore`, `GitHubUserStore`).
    pub issues: crate::autocomplete::IssueCaches,
    pub mentionables: crate::autocomplete::MentionableCaches,
    /// Open pull requests per GitHub repository (`PullRequestCoordinator`).
    pub pull_requests: crate::pull_requests::PullRequestCaches,
    /// `OnboardingTutorialAssessor.tutorialAnnounced` (per session).
    pub tutorial_announced: bool,
    /// `CORVENE_POPUP=tutorial:<step>`: the step the tutorial repository is
    /// shown at, whatever its state (dev/testing convenience).
    pub tutorial_step_override: Option<crate::tutorial::TutorialStep>,
    /// `CommitStatusStore`: CI statuses of refs.
    pub commit_statuses: crate::commit_status::CommitStatusStore,
    /// Corvene (`347-actions-job-logs`): Actions job logs fetched this session.
    pub job_logs: crate::job_log::JobLogStore,
    /// Corvene (`351-actions`): the open Actions view.
    pub actions: Option<crate::github_actions::ActionsViewState>,
    /// Corvene (`351-actions`): runs started from the view, watched until
    /// they finish.
    pub actions_watch: crate::github_actions::ActionsWatch,
    /// Corvene (`428-menu-bar-status-item`): what the indicator pass recorded
    /// for the watched repositories.
    pub menu_bar_statuses: crate::menu_bar_status::MenuBarStatuses,
    /// `cachedRepoRulesets`: ruleset id → the ruleset (how it applies to the user).
    pub repo_rulesets: HashMap<u64, corvene_github::ApiRepoRuleset>,
    /// Installed editors / shells (`getAvailableEditors` / `getAvailableShells`).
    pub editors: Vec<FoundEditor>,
    pub shells: Vec<FoundShell>,
    /// The icons of `editors` and `shells` by their `path` (flag
    /// `513-integration-app-icons`; empty while it is off).
    pub app_icons: HashMap<PathBuf, Arc<corvene_platform::app_icons::AppIcon>>,
    /// Loaded when the Settings dialog opens (`None` while loading).
    pub global_git: Option<GlobalGitConfig>,
    /// Loaded when the Repository Settings dialog opens.
    pub repo_settings: Option<RepositorySettingsData>,
    /// `resolveOpenInDesktop`: an `x-corvene://openRepo` action waiting for
    /// the clone it opened.
    pub pending_open_in_desktop: Option<crate::app_url::PendingOpenInDesktop>,
    /// `UpdateStore` state + `isUpdateAvailableBannerVisible`.
    pub update: crate::updater::UpdateState,
    /// On-demand packs.
    pub packs: crate::packs::PacksState,
    /// Language extensions (`crate::extensions`): installed grammars, the
    /// manager dialog's search and progress.
    pub extensions: crate::extensions::ExtensionsState,
    /// Alive subscriptions (`AliveStore`) and notification dedup state.
    pub alive: crate::alive::AliveState,
    /// `766-persist-commit-drafts`: each repository's unfinished commit
    /// message, and a counter that debounces writing them.
    pub commit_drafts: HashMap<u64, crate::drafts::CommitDraft>,
    pub commit_drafts_nonce: u64,
    /// `767-persist-file-selection`: each repository's unticked files as
    /// saved, and the repositories whose first status already got them.
    pub excluded_files: HashMap<u64, Vec<String>>,
    pub excluded_files_restored: std::collections::HashSet<u64>,
    /// `1310-file-list-tree`: each repository's collapsed folders in the
    /// changes tree, as saved.
    pub collapsed_folders: HashMap<u64, std::sync::Arc<std::collections::BTreeSet<String>>>,
    /// `1313-changelists`: each repository's changelists, as saved.
    pub changelists: HashMap<u64, std::sync::Arc<crate::changelists::Changelists>>,
    /// `117-file-icons`: the extension icon theme the setting names, loaded
    /// (`None` for the built-in set, none, or while it loads).
    pub file_icon_theme: Option<std::sync::Arc<crate::file_icons::LoadedIconTheme>>,
    /// Corvene (flags `342-gitlab`, `343-gitea`, `344-bitbucket`): GitLab,
    /// Gitea / Forgejo and Bitbucket accounts and data (`hosts.rs`).
    pub hosts: crate::hosts::HostsState,
}

/// `AppState` reads and writes the current workspace's slots as its own
/// (`s.selected`, `s.foldout`, `s.popups`, `s.banner`, `s.navigation`,
/// `s.drag_target`, `s.branches_tab`, `s.show_ci_status_popover`); see
/// `crate::workspace`.
impl std::ops::Deref for AppState {
    type Target = WorkspaceState;

    fn deref(&self) -> &WorkspaceState {
        &self.workspaces[self.current_index()]
    }
}

impl std::ops::DerefMut for AppState {
    fn deref_mut(&mut self) -> &mut WorkspaceState {
        let index = self.current_index();
        &mut self.workspaces[index]
    }
}

impl AppState {
    /// Where `current` is in `workspaces` (the first when it is gone;
    /// `workspaces` is never empty).
    fn current_index(&self) -> usize {
        self.workspaces
            .iter()
            .position(|w| w.id == self.current)
            .unwrap_or(0)
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&WorkspaceState> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut WorkspaceState> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    /// The workspace of the window last active (`current` outside a frame).
    pub fn focused_workspace(&self) -> &WorkspaceState {
        self.workspace(self.focused)
            .unwrap_or_else(|| &self.workspaces[0])
    }

    /// The repositories some window shows (each once, first workspace
    /// first): what the watchers, the pruners and the background fetch
    /// cover.
    pub fn visible_repositories(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = Vec::new();
        for w in &self.workspaces {
            if let Some(id) = w.selected
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        ids
    }

    /// The workspace whose window shows repository `id`, if any.
    pub fn workspace_showing(&self, id: u64) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|w| w.selected == Some(id))
            .map(|w| w.id)
    }

    /// Add a workspace (its window opens next); it becomes `current` and
    /// `focused`.
    pub fn add_workspace(&mut self, selected: Option<u64>, tabs: Vec<u64>) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let mut workspace = WorkspaceState::new(id, selected);
        if !tabs.is_empty() {
            workspace.tabs = tabs;
            if let Some(selected) = selected
                && !workspace.tabs.contains(&selected)
            {
                workspace.tabs.push(selected);
            }
        }
        self.workspaces.push(workspace);
        self.current = id;
        self.focused = id;
        id
    }

    /// Remove a workspace (its window closed); the last one stays. The
    /// cursors move to the first remaining workspace when they named it.
    pub fn remove_workspace(&mut self, id: WorkspaceId) -> bool {
        if self.workspaces.len() < 2 {
            return false;
        }
        let Some(index) = self.workspaces.iter().position(|w| w.id == id) else {
            return false;
        };
        self.workspaces.remove(index);
        let fallback = self.workspaces[0].id;
        if self.focused == id {
            self.focused = fallback;
        }
        if self.current == id {
            self.current = fallback;
        }
        true
    }

    /// What `ui.workspaces` stores.
    pub fn saved_workspaces(&self) -> Vec<SavedWorkspace> {
        self.workspaces.iter().map(WorkspaceState::saved).collect()
    }

    /// `429-multiple-windows` or `430-repository-tabs`: the workspace list
    /// is kept across launches.
    pub fn persists_workspaces(&self) -> bool {
        self.flags.bool(crate::flags::ids::MULTIPLE_WINDOWS)
            || self.flags.bool(crate::flags::ids::REPOSITORY_TABS)
    }

    /// Restart-required flags changed since launch (the Flags dialog's
    /// Relaunch bar).
    pub fn flags_restart_pending(&self) -> Vec<crate::flags::FlagId> {
        self.flags.restart_pending(&self.flags_at_launch)
    }

    /// `103-product-name`: what the Welcome flow and the tutorial call the app.
    pub fn product_name(&self) -> &str {
        self.flags.text(crate::flags::ids::PRODUCT_NAME)
    }

    /// GHD `currentPopup`: the popup on top of the stack, the one shown.
    pub fn popup(&self) -> Option<&Popup> {
        self.popups.current_popup().map(|p| &p.popup)
    }

    /// Where new repositories go: `Settings::clone_dir` (set in Settings ›
    /// Advanced, `514-default-clone-location`), else GitHub Desktop's
    /// default, outside OneDrive with `515-clone-dir-avoids-onedrive`.
    pub fn clone_dir(&self) -> std::path::PathBuf {
        self.settings.clone_dir.clone().unwrap_or_else(|| {
            corvene_platform::paths::default_clone_dir_avoiding_onedrive(
                self.flags
                    .bool(crate::flags::ids::CLONE_DIR_AVOIDS_ONEDRIVE),
            )
        })
    }

    /// `418-confirm-quit-while-busy`: the running operation quitting would
    /// cut short, if any.
    pub fn busy_for_quit(&self) -> Option<&'static str> {
        let network = self
            .repo_states
            .values()
            .any(|s| s.push_pull_in_progress && !s.quiet_background_fetch);
        busy_for_quit(
            !self.cloning.repositories().is_empty(),
            network,
            &self.update.status,
        )
    }
}

/// Describes the operation a quit would interrupt: a clone, a visible push /
/// pull / fetch, or an update being downloaded or installed (a quiet
/// background fetch doesn't count).
pub fn busy_for_quit(
    cloning: bool,
    network: bool,
    update: &crate::updater::UpdateStatus,
) -> Option<&'static str> {
    use crate::updater::UpdateStatus;
    if matches!(update, UpdateStatus::Installing) {
        Some("An update is being installed.")
    } else if cloning {
        Some("A repository is being cloned.")
    } else if network {
        Some("A push, pull or fetch is in progress.")
    } else if matches!(update, UpdateStatus::Downloading { .. }) {
        Some("An update is being downloaded.")
    } else {
        None
    }
}

impl AppState {
    /// The custom editor "Open in …" uses (Settings › Integrations): GHD's
    /// one, or with `523-custom-editor-list` the chosen one of the list.
    pub fn custom_editor_in_use(&self) -> Option<&crate::persistence::CustomIntegration> {
        if !self.settings.use_custom_editor {
            return None;
        }
        self.settings
            .chosen_custom_editor(self.flags.bool(crate::flags::ids::CUSTOM_EDITOR_LIST))
    }

    /// [`Self::custom_editor_in_use`]'s name in menus (its own with
    /// `508-custom-editor-name` or `523-custom-editor-list`).
    pub fn custom_editor_label(&self) -> Option<String> {
        let list = self.flags.bool(crate::flags::ids::CUSTOM_EDITOR_LIST);
        let custom = self.custom_editor_in_use()?;
        let index = if list {
            self.settings.custom_editor_index
        } else {
            0
        };
        let named = list || self.flags.bool(crate::flags::ids::CUSTOM_EDITOR_NAME);
        Some(custom.display_name(index, named))
    }

    /// The editor "Open in …" menu items name: the selected editor, else the
    /// first installed one, else GHD's generic "External Editor" (lower
    /// case off macOS, as GHD's non-darwin labels).
    pub fn editor_label(&self) -> String {
        // `518-per-repo-editor`: the selected repository's own editor
        if let Some(name) = self
            .selected
            .and_then(|id| self.repository(id))
            .and_then(|r| self.repository_editor(&r.path))
        {
            return name;
        }
        if let Some(custom) = self
            .selected
            .and_then(|id| self.repository(id))
            .and_then(|r| self.repository_custom_editor(&r.path))
        {
            return custom.display_name(0, true);
        }
        if let Some(label) = self.custom_editor_label() {
            return label;
        }
        self.settings
            .external_editor
            .clone()
            .or_else(|| self.editors.first().map(|e| e.name.clone()))
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    "External Editor"
                } else {
                    "external editor"
                }
                .to_string()
            })
    }

    /// The editor "Open in …" opens can jump to a line
    /// (`corvene_platform::editors::launch_at_line`; never a custom editor).
    pub fn editor_supports_line(&self) -> bool {
        if self.custom_editor_in_use().is_some() {
            return false;
        }
        corvene_platform::editors::find_editor_or_default(
            &self.editors,
            self.settings.external_editor.as_deref(),
        )
        .ok()
        .flatten()
        .is_some_and(corvene_platform::editors::supports_line)
    }

    /// The shell "Open in …" menu items name (`Terminal` by default).
    pub fn shell_label(&self) -> String {
        if self.settings.use_custom_shell && self.settings.custom_shell.is_some() {
            return "Custom Shell".to_string();
        }
        self.settings
            .shell
            .clone()
            .unwrap_or_else(|| corvene_platform::shells::DEFAULT_SHELL.label().to_string())
    }

    /// `518-per-repo-editor`: the installed editor chosen for the repository
    /// holding `path` (the innermost one), if any.
    /// `518-per-repo-editor`: the custom editor of the repository holding
    /// `path`, as Settings' custom editors are kept.
    pub fn repository_custom_editor(
        &self,
        path: &std::path::Path,
    ) -> Option<crate::persistence::CustomIntegration> {
        if !self.flags.bool(crate::flags::ids::PER_REPO_EDITOR) {
            return None;
        }
        let custom = self
            .repositories
            .iter()
            .filter(|r| path.starts_with(&r.path))
            .max_by_key(|r| r.path.components().count())?
            .custom_editor
            .clone()?;
        Some(crate::persistence::CustomIntegration {
            path: custom.path,
            arguments: custom.arguments,
            bundle_id: None,
            name: custom.name,
        })
    }

    pub fn repository_editor(&self, path: &std::path::Path) -> Option<String> {
        if !self.flags.bool(crate::flags::ids::PER_REPO_EDITOR) {
            return None;
        }
        let name = self
            .repositories
            .iter()
            .filter(|r| path.starts_with(&r.path))
            .max_by_key(|r| r.path.components().count())?
            .editor
            .clone()?;
        self.editors.iter().any(|e| e.name == name).then_some(name)
    }

    /// GHD `getAccountForEndpoint`: the endpoint's first account. Code that
    /// works for a repository asks [`Self::account_for_repository`].
    pub fn account_for(&self, endpoint: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.endpoint == endpoint)
    }

    /// Corvene (`527-multiple-accounts`): an endpoint can have several
    /// accounts and each repository uses one of them.
    pub fn multiple_accounts(&self) -> bool {
        self.flags.bool(crate::flags::ids::MULTIPLE_ACCOUNTS)
    }

    /// The accounts signed in to `endpoint`, in list order (one in GHD).
    pub fn accounts_for(&self, endpoint: &str) -> Vec<&Account> {
        self.accounts
            .iter()
            .filter(|a| a.endpoint == endpoint)
            .collect()
    }

    /// The account `login` signed in to `endpoint`.
    pub fn account_with_login(&self, endpoint: &str, login: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|a| a.endpoint == endpoint && a.login.eq_ignore_ascii_case(login))
    }

    /// GHD `getAccountForRepository` for repository `id` on `endpoint` (its
    /// GitHub repository's, or a parent's on the same host): with
    /// `527-multiple-accounts` the account the repository uses
    /// (`Repository::account`) while it is signed in, else the endpoint's
    /// first account.
    pub fn account_for_repository_on(&self, id: u64, endpoint: &str) -> Option<&Account> {
        self.chosen_account(id, endpoint)
            .or_else(|| self.account_for(endpoint))
    }

    /// [`Self::account_for_repository_on`] the endpoint of repository
    /// `id`'s GitHub repository; `None` for a repository not on GitHub.
    pub fn account_for_repository(&self, id: u64) -> Option<&Account> {
        let endpoint = self.repository(id)?.github.as_ref()?.endpoint.clone();
        self.account_for_repository_on(id, &endpoint)
    }

    /// The account to call the API about `github` with when no repository
    /// is at hand (commit status subscriptions, caches keyed by GitHub
    /// repository): the account of a listed repository that is `github` or
    /// a fork of it, the selected repository first; else the endpoint's
    /// first account (`527-multiple-accounts`).
    pub fn account_for_github(&self, github: &GitHubRepository) -> Option<&Account> {
        if self.multiple_accounts() {
            let same = |gh: &GitHubRepository| {
                gh.endpoint == github.endpoint
                    && gh.owner.eq_ignore_ascii_case(&github.owner)
                    && gh.name.eq_ignore_ascii_case(&github.name)
            };
            let matches = |r: &&Repository| {
                r.account.is_some()
                    && r.github
                        .as_ref()
                        .is_some_and(|gh| same(gh) || gh.parent.as_deref().is_some_and(same))
            };
            let found = self
                .selected_repository()
                .filter(matches)
                .or_else(|| self.repositories.iter().find(matches))
                .and_then(|r| self.chosen_account(r.id, &github.endpoint));
            if found.is_some() {
                return found;
            }
        }
        self.account_for(&github.endpoint)
    }

    /// The account repository `id` picked on `endpoint`, if it is still
    /// signed in and the flag is on.
    fn chosen_account(&self, id: u64, endpoint: &str) -> Option<&Account> {
        if !self.multiple_accounts() {
            return None;
        }
        let login = self.repository(id)?.account.as_deref()?;
        self.account_with_login(endpoint, login)
    }

    pub fn dotcom_account(&self) -> Option<&Account> {
        self.accounts.iter().find(|a| a.is_dotcom())
    }

    pub fn repository(&self, id: u64) -> Option<&Repository> {
        self.repositories.iter().find(|r| r.id == id)
    }

    pub fn selected_repository(&self) -> Option<&Repository> {
        self.selected.and_then(|id| self.repository(id))
    }

    pub fn selected_state(&self) -> Option<&RepositoryState> {
        self.selected.and_then(|id| self.repo_states.get(&id))
    }

    /// Corvene (`341-custom-autolinks`): the links of repository `id`'s
    /// own kinds of references in commit messages (`None`: GHD's only).
    pub fn link_rules(&self, id: u64) -> Option<Arc<[crate::text_tokens::LinkRule]>> {
        use crate::text_tokens::LinkRule;
        let mut rules = Vec::new();
        if self.flags.bool(crate::flags::ids::CUSTOM_AUTOLINKS) {
            let own = self.repository(id).map(|r| r.autolinks.as_slice());
            let api = self
                .repo_states
                .get(&id)
                .map(|rs| rs.api_autolinks.as_slice());
            rules.extend(
                own.into_iter()
                    .chain(api)
                    .flatten()
                    .filter(|a| !a.key_prefix.is_empty())
                    .map(|a| LinkRule::Autolink {
                        prefix: a.key_prefix.clone(),
                        url_template: a.url_template.clone(),
                        alphanumeric: a.is_alphanumeric,
                    }),
            );
        }
        // `1211-issuetracker-links`
        if self.flags.bool(crate::flags::ids::ISSUETRACKER_LINKS)
            && let Some(rs) = self.repo_states.get(&id)
        {
            rules.extend(rs.issue_trackers.iter().cloned());
        }
        (!rules.is_empty()).then(|| rules.into())
    }

    /// What locks flag `id` in the Flags dialog: the settings file
    /// (`522-settings-file`) or `CORVENE_FLAGS`.
    pub fn flag_lock_source(&self, id: Option<crate::flags::FlagId>) -> &'static str {
        let from_file = match id {
            Some(id) => self.settings_file_flags.values.contains_key(&id),
            None => self.settings_file_flags.preset.is_some(),
        };
        if from_file {
            crate::settings_file::FILE_NAME
        } else {
            crate::flags::env::VAR
        }
    }

    /// Where the user is (`427-back-forward-navigation`): the selected
    /// repository and its section.
    pub fn navigation_entry(&self) -> Option<crate::navigation::NavigationEntry> {
        let repository = self.selected?;
        Some(crate::navigation::NavigationEntry {
            repository,
            section: self
                .repo_states
                .get(&repository)
                .map(|rs| rs.section)
                .unwrap_or_default(),
        })
    }

    /// The user is about to leave [`Self::navigation_entry`]: a Back step
    /// (`427-back-forward-navigation`).
    pub(crate) fn record_navigation(&mut self) {
        if !self.flags.bool(crate::flags::ids::BACK_FORWARD_NAVIGATION) {
            return;
        }
        if let Some(entry) = self.navigation_entry() {
            self.navigation.record(entry);
        }
    }

    pub fn repo_state_mut(&mut self, id: u64) -> &mut RepositoryState {
        self.repo_states.entry(id).or_default()
    }

    /// GHD `GitStoreCache.remove(repository)`: forget the repository's
    /// state, so the next [`Self::repo_state_mut`] starts afresh
    /// (`Dispatcher::remove_repository`).
    pub fn remove_repo_state(&mut self, id: u64) {
        self.repo_states.remove(&id);
    }

    /// GHD `RepositoriesStore`: the repository list and its persistence.
    /// `291-recent-worktrees`: the repository is in use at its current path.
    pub fn record_recent_worktree(&mut self, id: u64) {
        let Some(path) = self.repository(id).map(|r| r.path.clone()) else {
            return;
        };
        if self.recent_worktrees.first() == Some(&(id, path.clone())) {
            return;
        }
        self.recent_worktrees
            .retain(|(r, p)| !(*r == id && *p == path));
        self.recent_worktrees.insert(0, (id, path));
        self.recent_worktrees.truncate(RECENT_WORKTREES_LENGTH);
        self.save_recent_worktrees();
    }

    /// `291-recent-worktrees`: forget the worktree `path` of `id` (`None`:
    /// all of its worktrees).
    pub fn forget_recent_worktrees(&mut self, id: u64, path: Option<&Path>) {
        let before = self.recent_worktrees.len();
        self.recent_worktrees
            .retain(|(r, p)| !(*r == id && path.is_none_or(|path| p == path)));
        if self.recent_worktrees.len() != before {
            self.save_recent_worktrees();
        }
    }

    fn save_recent_worktrees(&self) {
        use crate::persistence::StoreExt;
        if let Err(err) = self.store.save_recent_worktrees(&self.recent_worktrees) {
            tracing::warn!(%err, "could not save the recent worktrees");
        }
    }

    pub fn repositories_store(&mut self) -> crate::repositories_store::RepositoriesStore<'_> {
        crate::repositories_store::RepositoriesStore::new(&self.store, &mut self.repositories)
    }

    /// Repositories ordered for the foldout: alphabetical by display name.
    pub fn sorted_repositories(&self) -> Vec<&Repository> {
        let mut v: Vec<&Repository> = self.repositories.iter().collect();
        v.sort_by_key(|r| r.name().to_lowercase());
        v
    }
}

#[cfg(test)]
mod popup_tests {
    use super::Popup;

    #[test]
    fn repository_bound_popups_name_their_repository() {
        let rename = Popup::RenameBranch {
            repo: 7,
            name: "main".into(),
        };
        assert_eq!(rename.repository(), Some(7));
        assert_eq!(Popup::Acknowledgements.repository(), None);
    }

    #[test]
    fn quit_confirmation_names_the_running_operation() {
        use super::busy_for_quit;
        use crate::updater::UpdateStatus;
        assert_eq!(
            busy_for_quit(false, false, &UpdateStatus::NotAvailable),
            None
        );
        assert_eq!(busy_for_quit(false, false, &UpdateStatus::Checking), None);
        assert!(
            busy_for_quit(true, false, &UpdateStatus::NotChecked)
                .is_some_and(|s| s.contains("cloned"))
        );
        assert!(
            busy_for_quit(false, true, &UpdateStatus::NotChecked)
                .is_some_and(|s| s.contains("push"))
        );
        assert!(
            busy_for_quit(true, true, &UpdateStatus::Installing)
                .is_some_and(|s| s.contains("installed"))
        );
    }
}
