//! The open dialog (`corvene_core::state::Popup`) as one record: the
//! variant's name (GHD's `PopupType` name, which the Kotlin `PopupHost`
//! switches on) plus its payload flattened to key/value strings. A typed
//! mirror of 75 variants would double the surface for no gain; the few
//! payloads that are whole structs are flattened to what a dialog shows.

use corvene_core::AppState;
use corvene_core::state::Popup;

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct PopupVm {
    /// The variant name: `ConfirmDiscardChanges`, `DeleteBranch`, …
    pub kind: String,
    pub repo: Option<u64>,
    pub fields: Vec<KeyValue>,
    /// Lists in the payload (`paths`, `tags`, `files`, `usernames`, …), as
    /// `key` → items.
    pub lists: Vec<KeyList>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct KeyList {
    pub key: String,
    pub items: Vec<String>,
}

struct Fields {
    fields: Vec<KeyValue>,
    lists: Vec<KeyList>,
}

impl Fields {
    fn new() -> Self {
        Fields {
            fields: Vec::new(),
            lists: Vec::new(),
        }
    }
    fn put(&mut self, key: &str, value: impl ToString) -> &mut Self {
        self.fields.push(KeyValue {
            key: key.to_string(),
            value: value.to_string(),
        });
        self
    }
    fn opt(&mut self, key: &str, value: Option<impl ToString>) -> &mut Self {
        if let Some(value) = value {
            self.put(key, value);
        }
        self
    }
    fn path(&mut self, key: &str, value: &std::path::Path) -> &mut Self {
        self.put(key, value.to_string_lossy())
    }
    fn list(&mut self, key: &str, items: impl IntoIterator<Item = impl ToString>) -> &mut Self {
        self.lists.push(KeyList {
            key: key.to_string(),
            items: items.into_iter().map(|i| i.to_string()).collect(),
        });
        self
    }
}

/// The variant name from the `Debug` form (`Kind { .. }` / `Kind(..)` / `Kind`).
fn kind_name(popup: &Popup) -> String {
    let debug = format!("{popup:?}");
    debug
        .split([' ', '{', '('])
        .next()
        .unwrap_or(&debug)
        .to_string()
}

pub fn popup(s: &AppState) -> Option<PopupVm> {
    let popup = s.popup()?;
    let mut f = Fields::new();
    let mut repo = None;
    match popup {
        Popup::InstallGit { reason } => {
            f.put("reason", reason);
        }
        Popup::Error {
            title,
            message,
            git,
        } => {
            // `text`: what the dialog body says, as the desktop derives it
            // (the message, else the git failure's explanation, else its
            // first line); `details`: the full failure behind "Show details"
            let text = if !message.is_empty() {
                message.clone()
            } else if let Some(git) = git {
                git.lead("Settings")
                    .or_else(|| git.description("Settings"))
                    .unwrap_or_else(|| git.output.lines().next().unwrap_or("").to_string())
            } else {
                String::new()
            };
            f.put("title", title)
                .put("message", message)
                .put("text", text);
            if let Some(git) = git {
                f.put("command", &git.command)
                    .opt("exit_code", git.exit_code)
                    .put("output", &git.output)
                    .put("details", git.summary());
            }
        }
        Popup::IndexLockExists {
            title,
            message,
            lock,
        } => {
            f.put("title", title)
                .put("message", message)
                .path("lock", lock);
        }
        // hooks interception (`corvene_git::hooks`) never runs on Android
        Popup::HookFailed {
            hook_name,
            terminal_output,
            ..
        } => {
            f.put("hook_name", hook_name)
                .put("terminal_output", terminal_output);
        }
        Popup::CommitProgress { .. } => {}
        Popup::AddExistingRepository { path } | Popup::CreateRepository { path } => {
            f.opt(
                "path",
                path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            );
        }
        Popup::CloneRepository { url } => {
            f.opt("url", url.as_ref());
        }
        Popup::CloneRepositoryRetry { url, path, error } => {
            f.put("url", url).path("path", path).put("error", error);
        }
        Popup::SignIn { enterprise } => {
            f.put("enterprise", enterprise);
        }
        // flags 342-344 (the Compose app has no host sign-in dialog yet)
        Popup::SignInHost { kind } => {
            f.put("kind", kind.name());
        }
        Popup::DiscardChanges {
            repo: r,
            paths,
            all,
        } => {
            repo = Some(*r);
            f.put("all", all).list("paths", paths);
        }
        Popup::CheckoutFromFork { repo: r, input } => {
            repo = Some(*r);
            f.put("input", input);
        }
        Popup::InvalidatedToken { account } => {
            f.put("login", &account.login).put("host", account.host());
        }
        Popup::StartPullRequest { repo: r }
        | Popup::CreateFork { repo: r }
        | Popup::AddLicense { repo: r }
        | Popup::ChooseForkSettings { repo: r }
        | Popup::TestNotifications { repo: r }
        | Popup::WarnLocalChangesBeforeUndo { repo: r }
        | Popup::ChangeRepositoryAlias { repo: r }
        | Popup::MoveRepositoryToGroup { repo: r }
        | Popup::ConfirmDiscardStash { repo: r }
        | Popup::StashWithMessage { repo: r }
        | Popup::NewChangelist { repo: r, .. }
        | Popup::CleanUntrackedFiles { repo: r }
        | Popup::Submodules { repo: r }
        | Popup::SparseCheckout { repo: r }
        | Popup::PublishRepository { repo: r }
        | Popup::PushNeedsPull { repo: r }
        | Popup::ConfirmRemoveRepository { repo: r } => {
            repo = Some(*r);
        }
        Popup::CLIInstalled { path } => {
            f.path("path", path);
        }
        Popup::MoveToApplicationsFolder
        | Popup::Acknowledgements
        | Popup::ImportFromGitHubDesktop
        | Popup::CreateSshKey
        | Popup::CommandPalette
        | Popup::EditLine { .. }
        | Popup::ConfirmDiscardFileEdit { .. }
        | Popup::ConfirmExitTutorial => {}
        Popup::CrashReportFound { reports } => {
            f.list("reports", reports.iter().map(|p| p.to_string_lossy()));
        }
        Popup::ReleaseNotes { summary } => {
            f.put("summary", format!("{summary:?}"));
        }
        Popup::UpstreamAlreadyExists {
            repo: r,
            existing_url,
        } => {
            repo = Some(*r);
            f.put("existing_url", existing_url);
        }
        Popup::PushProtectionError {
            repo: r,
            secrets,
            bypassed,
        } => {
            repo = Some(*r);
            f.list("secrets", secrets.iter().map(|s| &s.description))
                .list("bypassed", bypassed);
        }
        Popup::BypassPushProtection {
            repo: r,
            secret,
            secrets,
            bypassed,
        } => {
            repo = Some(*r);
            f.put("secret", &secret.description)
                .put("bypass_url", &secret.bypass_url)
                .list("secrets", secrets.iter().map(|s| &s.description))
                .list("bypassed", bypassed);
        }
        Popup::PushRejectedDueToMissingWorkflowScope {
            repo: r,
            rejected_path,
        } => {
            repo = Some(*r);
            f.put("rejected_path", rejected_path);
        }
        Popup::SAMLReauthRequired {
            repo: r,
            organization,
            endpoint,
            retry,
        } => {
            repo = Some(*r);
            f.put("organization", organization)
                .put("endpoint", endpoint)
                .opt("retry", retry.as_ref().map(|r| format!("{r:?}")));
        }
        Popup::CreateTutorialRepository { account, progress } => {
            f.put("login", &account.login);
            if let Some((title, percent, description)) = progress {
                f.put("progress_title", title)
                    .put("progress_percent", percent)
                    .opt("progress_description", description.as_ref());
            }
        }
        Popup::ImportGitConfig { settings, skipped } => {
            f.put("skipped", skipped)
                .list("settings", settings.iter().map(|(k, v)| format!("{k}={v}")));
        }
        Popup::SshKeyPassphrase { path, wrong } => {
            f.path("path", path).put("wrong", wrong);
        }
        Popup::SshKeyNeedsScope {
            endpoint,
            login,
            title,
        } => {
            f.put("endpoint", endpoint)
                .put("login", login)
                .put("title", title);
        }
        Popup::CICheckRunRerun {
            repo: r,
            github,
            checks,
            git_ref,
            failed_only,
        } => {
            repo = Some(*r);
            f.put("github", format!("{}/{}", github.owner, github.name))
                .put("git_ref", git_ref)
                .put("failed_only", failed_only)
                .list("checks", checks.iter().map(|c| &c.name));
        }
        Popup::PullRequestReview {
            repo: r,
            pull_request,
            review,
            should_checkout_branch,
            should_change_repository,
        } => {
            repo = Some(*r);
            f.put("number", pull_request.number)
                .put("title", &pull_request.title)
                .put("author", &pull_request.author)
                .put("review", format!("{review:?}"))
                .put("should_checkout_branch", should_checkout_branch)
                .put("should_change_repository", should_change_repository);
        }
        Popup::PullRequestComment {
            repo: r,
            pull_request,
            comment,
            should_checkout_branch,
            should_change_repository,
        } => {
            repo = Some(*r);
            f.put("number", pull_request.number)
                .put("title", &pull_request.title)
                .put("author", &pull_request.author)
                .put("comment", format!("{comment:?}"))
                .put("should_checkout_branch", should_checkout_branch)
                .put("should_change_repository", should_change_repository);
        }
        Popup::PullRequestChecksFailed {
            repo: r,
            pull_request,
            checks,
            should_change_repository,
        } => {
            repo = Some(*r);
            f.put("number", pull_request.number)
                .put("title", &pull_request.title)
                .put("should_change_repository", should_change_repository)
                .list("checks", checks.iter().map(|c| &c.name));
        }
        Popup::UnknownAuthors {
            repo: r,
            usernames,
            summary,
            description,
        } => {
            repo = Some(*r);
            f.put("summary", summary)
                .put("description", description)
                .list("usernames", usernames);
        }
        Popup::OversizedFiles {
            repo: r,
            files,
            summary,
            description,
            lfs_patterns,
            ..
        } => {
            repo = Some(*r);
            f.put("summary", summary)
                .put("description", description)
                .list("files", files)
                .list("lfs_patterns", lfs_patterns);
        }
        Popup::AddEmbeddedRepositories {
            repo: r,
            repositories,
            commit,
        } => {
            repo = Some(*r);
            f.opt("summary", commit.as_ref().map(|c| &c.0))
                .opt("description", commit.as_ref().map(|c| &c.1))
                .list("paths", repositories.iter().map(|r| &r.path));
        }
        Popup::CommitToNewBranch {
            repo: r,
            summary,
            description,
        } => {
            repo = Some(*r);
            f.put("summary", summary).put("description", description);
        }
        Popup::CreateBranchFromCommits { repo: r, plan } => {
            repo = Some(*r);
            f.put("removable", plan.move_back.is_some())
                .list("commits", &plan.commits);
        }
        Popup::ConfirmCommitToDefaultBranch {
            repo: r,
            branch,
            summary,
            description,
            unknown_co_authors,
        } => {
            repo = Some(*r);
            f.put("branch", branch)
                .put("summary", summary)
                .put("description", description)
                .list("unknown_co_authors", unknown_co_authors);
        }
        Popup::ConfirmDiscardSelection {
            repo: r,
            path,
            selection,
        } => {
            repo = Some(*r);
            f.put("path", path)
                .put("selection", format!("{:?}", selection.kind()));
        }
        Popup::ResetToCommit { repo: r, sha }
        | Popup::CheckoutCommit { repo: r, sha }
        | Popup::CreateTag { repo: r, sha } => {
            repo = Some(*r);
            f.put("sha", sha);
        }
        Popup::ConfirmForceUnlock {
            repo: r,
            path,
            owner,
        } => {
            repo = Some(*r);
            f.put("path", path).put("owner", owner);
        }
        Popup::ApplyPatch {
            repo: r,
            name,
            preview,
            ..
        } => {
            repo = Some(*r);
            f.put("name", name)
                .put("mailbox", preview.is_mailbox())
                .put("applies_cleanly", preview.applies_cleanly)
                .list("files", preview.files.iter().map(|file| file.path.clone()))
                .list("commits", preview.commits.clone());
        }
        Popup::ResetToReflogEntry {
            repo: r,
            sha,
            branch,
            dirty,
        } => {
            repo = Some(*r);
            f.put("sha", sha)
                .opt("branch", branch.clone())
                .put("dirty", dirty);
        }
        Popup::StartBisect {
            repo: r,
            mark,
            branch,
        } => {
            repo = Some(*r);
            f.put("branch", branch).opt(
                "mark",
                mark.as_ref()
                    .map(|(verdict, sha)| format!("{verdict:?} {sha}")),
            );
        }
        Popup::ResetToRemote {
            repo: r,
            branch,
            upstream,
            ahead,
            dirty,
            ..
        } => {
            repo = Some(*r);
            f.put("branch", branch)
                .put("upstream", upstream)
                .put("ahead", ahead)
                .put("dirty", dirty);
        }
        Popup::ConfirmDeletePushedTag {
            repo: r,
            tag,
            remote_only,
        } => {
            repo = Some(*r);
            f.put("tag", tag).put("remote_only", remote_only);
        }
        Popup::WarnTaggedCommitBeforeUndo {
            repo: r,
            tags,
            warn_local,
        } => {
            repo = Some(*r);
            f.put("warn_local", warn_local).list("tags", tags);
        }
        Popup::CreateBranch {
            repo: r,
            target_sha,
            initial_name,
        } => {
            repo = Some(*r);
            f.opt("target_sha", target_sha.as_ref())
                .put("initial_name", initial_name);
        }
        Popup::RenameBranch { repo: r, name } | Popup::DeleteBranch { repo: r, name } => {
            repo = Some(*r);
            f.put("name", name);
        }
        Popup::AddWorktree {
            repo: r,
            initial_branch_name,
            initial_worktree_name,
        } => {
            repo = Some(*r);
            f.opt("initial_branch_name", initial_branch_name.as_ref())
                .opt("initial_worktree_name", initial_worktree_name.as_ref());
        }
        Popup::RenameWorktree { repo: r, path } | Popup::DeleteWorktree { repo: r, path } => {
            repo = Some(*r);
            f.path("path", path);
        }
        Popup::DeleteWorktreeFailed {
            repo: r,
            path,
            error,
            original,
        } => {
            repo = Some(*r);
            f.path("path", path).put("error", error).opt(
                "original",
                original.as_ref().map(|p| p.to_string_lossy().into_owned()),
            );
        }
        Popup::StashAndSwitchBranch { repo: r, branch }
        | Popup::ConfirmSwitchBranch { repo: r, branch } => {
            repo = Some(*r);
            f.put("branch", branch);
        }
        Popup::ConfirmOverwriteStash { repo: r, branch } => {
            repo = Some(*r);
            f.opt("branch", branch.clone());
        }
        Popup::MergeBranch { repo: r, squash } => {
            repo = Some(*r);
            f.put("squash", squash);
        }
        Popup::MultiCommitOperation { repo: r, flow } => {
            repo = Some(*r);
            f.put("flow", flow);
        }
        Popup::LocalChangesOverwritten {
            repo: r,
            retry,
            files,
        } => {
            repo = Some(*r);
            f.put("retry", format!("{retry:?}")).list("files", files);
        }
        Popup::PushBranchCommits {
            repo: r,
            branch,
            unpushed,
            base,
        } => {
            repo = Some(*r);
            f.put("branch", branch)
                .opt("unpushed", *unpushed)
                .opt("base", base.as_ref());
        }
        Popup::ConfirmForcePush {
            repo: r,
            upstream_branch,
        } => {
            repo = Some(*r);
            f.put("upstream_branch", upstream_branch);
        }
        Popup::ConfirmPushToUpstream {
            repo: r,
            branch,
            upstream,
            remote_branch,
            force_with_lease,
            up_to,
        } => {
            repo = Some(*r);
            f.put("branch", branch)
                .put("upstream", upstream)
                .put("remote_branch", remote_branch)
                .put("force_with_lease", force_with_lease.to_string())
                .opt("up_to", up_to.as_ref());
        }
        Popup::GenericGitAuthentication {
            repo: r,
            remote_url,
            host,
            username,
            retry,
        } => {
            repo = Some(*r);
            f.put("remote_url", remote_url)
                .put("host", host)
                .opt("username", username.as_ref())
                .put("retry", format!("{retry:?}"));
        }
        Popup::ProxyAuthentication {
            proxy,
            server,
            username,
            rejected,
            retry,
        } => {
            f.put("proxy", proxy)
                .put("server", server)
                .opt("username", username.as_ref())
                .put("rejected", rejected)
                .put("retry", format!("{retry:?}"));
        }
        Popup::InitializeLFS { repos } => {
            f.list("repos", repos);
        }
        Popup::SquashCommitMessage {
            repo: r,
            to_squash,
            onto,
            summary,
            description,
            count,
        } => {
            repo = Some(*r);
            f.put("onto", onto)
                .put("summary", summary)
                .put("description", description)
                .put("count", count)
                .list("to_squash", to_squash);
        }
        // `1221-stacked-branch-refs`
        Popup::WarnStackedBranches {
            repo: r,
            branches,
            update_refs: _,
            op,
        } => {
            repo = Some(*r);
            f.put("op", format!("{op:?}")).list("branches", branches);
        }
        // `1116-ssh-prompts`: the Android port runs no ssh
        Popup::AddSshHost { request, info: _ } => {
            f.put("request", *request);
        }
        Popup::SshSecret {
            request,
            kind: _,
            subject,
            wrong,
        } => {
            f.put("request", *request)
                .put("subject", subject)
                .put("wrong", *wrong);
        }
        // `356-stacked-pull-requests`
        Popup::CreateStackedPullRequests { repo: r } => {
            repo = Some(*r);
        }
        Popup::Preferences { tab } => {
            f.put("tab", format!("{tab:?}"));
        }
        Popup::Flags { query } => {
            f.opt("query", query.as_ref());
        }
        Popup::LanguageExtensions { focus, return_to } => {
            f.opt("focus", focus.as_ref().map(|x| format!("{x:?}")))
                .opt("return_to", return_to.as_ref().map(|x| format!("{x:?}")));
        }
        Popup::RepositorySettings { repo: r, tab } => {
            repo = Some(*r);
            f.put("tab", format!("{tab:?}"));
        }
        Popup::About { version } => {
            f.put("version", version);
        }
        Popup::ExternalEditorError {
            message,
            suggest_default_editor,
            open_preferences,
            ..
        } => {
            f.put("message", message)
                .put("suggest_default_editor", suggest_default_editor)
                .put("open_preferences", open_preferences);
        }
        Popup::ShellError { message, .. } => {
            f.put("message", message);
        }
        Popup::UnreachableCommits { repo: r, tab } => {
            repo = Some(*r);
            f.put("tab", format!("{tab:?}"));
        }
        Popup::RemoveRepositories { ticked } => {
            f.opt("ticked", *ticked);
        }
        Popup::ConfirmDeleteUntrashable { repo: r, paths } => {
            repo = Some(*r);
            f.list("paths", paths);
        }
        Popup::IgnoreWithPattern { repo: r, pattern } => {
            repo = Some(*r);
            f.put("pattern", pattern);
        }
        Popup::WarnTaggedCommitBeforeAmend { repo: r, sha, tags } => {
            repo = Some(*r);
            f.put("sha", sha).list("tags", tags);
        }
        Popup::MoveChangesToWorktree { repo: r } => {
            repo = Some(*r);
        }
        Popup::DropKeptStash { repo: r, stash }
        | Popup::ConfirmDropStashEntry { repo: r, stash }
        | Popup::CreateBranchFromStash { repo: r, stash } => {
            repo = Some(*r);
            f.put("stash", &stash.sha);
        }
        Popup::RequestReviewers { repo: r, number } => {
            repo = Some(*r);
            f.put("number", number);
        }
        Popup::NewIssue { repo: r } => {
            repo = Some(*r);
        }
        Popup::SubmitPullRequestReview { repo: r } => {
            repo = Some(*r);
        }
        Popup::ActionsJobLog {
            repo: r,
            check,
            step,
            ..
        } => {
            repo = *r;
            f.put("check", &check.name);
            f.opt("step", step.as_deref());
        }
        Popup::CompareRefs {
            repo: r,
            base,
            head,
        } => {
            repo = Some(*r);
            f.opt("base", base.as_deref());
            f.opt("head", head.as_deref());
        }
        Popup::CreateRelease { repo: r, tag, sha } => {
            repo = Some(*r);
            f.opt("tag", tag.as_deref());
            f.opt("sha", sha.as_deref());
        }
        Popup::DeleteBranches { repo: r, names } => {
            repo = Some(*r);
            f.list("names", names);
        }
        Popup::ChooseRepositoryAccount { repo: r, logins } => {
            repo = Some(*r);
            f.list("logins", logins);
        }
        Popup::MoveToSharedStorage { repo: r, then } => {
            repo = Some(*r);
            f.put("then", format!("{then:?}"));
        }
        Popup::ConfirmQuit { busy } => {
            f.put("busy", busy);
        }
        Popup::Actions { github, repo: r } => {
            repo = *r;
            f.put("repository", github.full_name());
        }
        Popup::RunWorkflow { github, workflow } => {
            f.put("repository", github.full_name());
            f.put("workflow", workflow);
        }
    }
    Some(PopupVm {
        kind: kind_name(popup),
        repo,
        fields: f.fields,
        lists: f.lists,
    })
}

/// The banner under the toolbar (merge/rebase outcomes, conflicts…), as
/// its kind plus the payload flattened like [`PopupVm`].
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BannerVm {
    pub kind: String,
    pub text: String,
    pub nonce: u64,
    pub repo: Option<u64>,
    pub fields: Vec<KeyValue>,
}

pub fn banner(s: &AppState) -> Option<BannerVm> {
    use corvene_core::mco::Banner;
    let banner = s.banner.as_ref()?;
    let debug = format!("{banner:?}");
    let kind = debug
        .split([' ', '{', '('])
        .next()
        .unwrap_or(&debug)
        .to_string();
    let mut f = Fields::new();
    let mut repo = None;
    match banner {
        Banner::SuccessfulMerge {
            our_branch,
            their_branch,
        }
        | Banner::BranchAlreadyUpToDate {
            our_branch,
            their_branch,
        } => {
            f.put("our_branch", our_branch)
                .opt("their_branch", their_branch.as_ref());
        }
        Banner::SuccessfulRebase {
            target_branch,
            base_branch,
        } => {
            f.put("target_branch", target_branch)
                .opt("base_branch", base_branch.as_ref());
        }
        Banner::SuccessfulCherryPick {
            repo: r,
            target_branch,
            count,
            undoable,
        } => {
            repo = Some(*r);
            f.put("target_branch", target_branch)
                .put("count", count)
                .put("undoable", undoable);
        }
        Banner::BranchesDeleted { repo: r, branches } => {
            repo = Some(*r);
            f.list(
                "branches",
                branches.iter().map(|(name, sha)| format!("{name}\t{sha}")),
            );
        }
        Banner::BranchesRestored { count } => {
            f.put("count", count);
        }
        Banner::GitEmailMismatch { host, missing } => {
            f.put("host", host).put("missing", missing);
        }
        Banner::RepositoriesUnreadable { count, raw } => {
            f.put("count", count).put("raw", raw);
        }
        Banner::TemporaryStore { path } => {
            f.put("path", path.display());
        }
        Banner::ConfigFileErrors { path, errors } => {
            f.put("path", path.display());
            f.list("errors", errors.iter().cloned());
        }
        Banner::RepositoryMoved { name, path } => {
            f.put("name", name).put("path", path.display());
        }
        Banner::CherryPickUndone {
            target_branch,
            count,
        } => {
            f.put("target_branch", target_branch).put("count", count);
        }
        Banner::SuccessfulSquash { repo: r, count }
        | Banner::SuccessfulReorder { repo: r, count } => {
            repo = Some(*r);
            f.put("count", count);
        }
        Banner::SquashUndone { count } | Banner::ReorderUndone { count } => {
            f.put("count", count);
        }
        Banner::FixupCommitted { repo: r, target } => {
            repo = Some(*r);
            f.put("target", target);
        }
        // `345-issues` / `346-releases`
        Banner::IssueCreated { number, html_url } => {
            f.put("number", number).put("html_url", html_url);
        }
        Banner::ReviewSubmitted {
            number,
            event,
            html_url,
        } => {
            f.put("number", number)
                .put("html_url", html_url)
                .put("event", event.graphql());
        }
        Banner::ReleaseCreated {
            name,
            html_url,
            draft,
        } => {
            f.put("name", name)
                .put("html_url", html_url)
                .put("draft", draft);
        }
        Banner::BranchDeleted {
            repo: r,
            branch,
            sha,
        } => {
            repo = Some(*r);
            f.put("branch", branch).put("sha", sha);
        }
        Banner::BranchRestored { branch } => {
            f.put("branch", branch);
        }
        Banner::StashDropped {
            repo: r,
            sha,
            message,
        } => {
            repo = Some(*r);
            f.put("sha", sha).put("message", message);
        }
        Banner::StashRestored | Banner::SecurityKeyTouch { .. } => {}
        // `356-stacked-pull-requests`
        Banner::StackBaseMerged { repo: r, .. } | Banner::StackUpdated { repo: r } => {
            repo = Some(*r);
        }
        Banner::StackPushing { .. }
        | Banner::StackPushed { .. }
        | Banner::StackedPullRequestsCreated { .. } => {}
        Banner::StashFileDiscarded {
            repo: r,
            path,
            sha,
            message,
            position,
            replacing,
        } => {
            repo = Some(*r);
            f.put("path", path)
                .put("sha", sha)
                .put("message", message)
                .put("position", position.to_string())
                .put("replacing", replacing.as_deref().unwrap_or(""));
        }
        Banner::PatchApplied {
            files,
            commits,
            conflicts,
        } => {
            f.put("files", files)
                .put("commits", commits)
                .put("conflicts", conflicts);
        }
        Banner::ConflictsFound {
            repo: r,
            description,
            branch,
        } => {
            repo = Some(*r);
            f.put("description", description)
                .opt("branch", branch.as_ref());
        }
    }
    Some(BannerVm {
        kind,
        text: debug,
        nonce: s.banner_nonce,
        repo,
        fields: f.fields,
    })
}
