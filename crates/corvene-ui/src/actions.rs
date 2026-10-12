//! Every menu / keyboard action, named after GitHub Desktop's menu items.

gpui_kit::actions!(
    corvene,
    [
        // Lists (arrow keys while the changes list has focus)
        SelectNextFile,
        SelectPreviousFile,
        SelectAllFiles,
        // ⇧↑ / ⇧↓ range selection in multi-select lists
        ExtendSelectionUp,
        ExtendSelectionDown,
        // ⌘↑ / ⌘↓ (Home / End): the first / last row (GHD `List` isHomeKey / isEndKey)
        SelectFirstFile,
        SelectLastFile,
        // Space in the changes list: include / exclude the highlighted files
        ToggleIncludeSelected,
        // ← / → on a file tree's row (`1310-file-list-tree`)
        CollapseFolder,
        ExpandFolder,
        // `1323-archive-diff`: Return opens an archive inside the archive,
        // ⌫ goes back out
        OpenArchiveEntry,
        ArchiveUp,
        // View › Show Changes as Tree / as List (`1310-file-list-tree`)
        ToggleFileListTree,
        // ⌘⌫ in the changes list (`605-cmd-backspace-discards-files`)
        DiscardSelectedFiles,
        // ⇧⌘A / ⌥⌘O in a file list (`606-open-file-shortcuts`)
        OpenSelectedFileInEditor,
        OpenSelectedFileWithDefaultProgram,
        // ⌥⌘S: unified ⇄ split diff (`610-diff-mode-shortcut`)
        ToggleDiffDisplayMode,
        // View › Wrap Diff Lines (`1304-diff-no-wrap`)
        ToggleDiffWordWrap,
        // The tab's stack button (`1311-stacked-diff`): every file's diff in one list
        ToggleStackedDiff,
        // ⌥⌘C / ⇧⌥⌘C in a file list (`611-copy-path-shortcuts`)
        CopySelectedFilePaths,
        CopySelectedRelativeFilePaths,
        // `612-navigation-shortcuts`
        ShowPullRequestsList,
        NextRepository,
        PreviousRepository,
        FocusDiff,
        SelectNextFileFromDiff,
        SelectPreviousFileFromDiff,
        // View › Toggle History Review Mode (`801-history-review-mode`)
        ToggleHistoryReviewMode,
        // Shift+F10 / the Menu key: the selected row's context menu
        OpenRowContextMenu,
        // ← / → between the lists and the diff (`619-arrow-keys-between-panes`)
        FocusPaneLeft,
        FocusPaneRight,
        // Tab / Shift+Tab (`keyboard_nav`)
        FocusNext,
        FocusPrevious,
        // View › Back / Forward (`427-back-forward-navigation`)
        NavigateBack,
        NavigateForward,
        // Worktrees
        NewWorktree,
        ShowWorktreesList,
        // Commit form: "Add Co-Authors" / "Remove Co-Authors"
        ToggleCoAuthors,
        // Commit form context menu (spelling suggestions are picked by index)
        SpellSuggestion0,
        SpellSuggestion1,
        SpellSuggestion2,
        SpellSuggestion3,
        SpellSuggestion4,
        SpellAddToDictionary,
        ToggleCommitSpellcheck,
        // Compare-to-branch filter box
        CompareSelect,
        CompareClear,
        // Enter in a foldout's filter box: pick the highlighted (or first) row
        FilterListPick,
        // History keyboard reorder mode
        ReorderMoveUp,
        ReorderMoveDown,
        ReorderConfirm,
        ReorderCancel,
        // App menu
        About,
        OpenSettings,
        OpenFlags,
        InstallCli,
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        // File
        NewRepository,
        AddLocalRepository,
        CloneRepository,
        ImportFromGitHubDesktop,
        // File › Remove Repositories… (`269-bulk-remove-repositories`)
        RemoveRepositories,
        // Edit
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll,
        Find,
        // View
        ShowChanges,
        ShowHistory,
        ShowRepositoryList,
        ShowBranchesList,
        GoToSummary,
        ToggleStashedChanges,
        ToggleChangesFilter,
        ToggleFullScreen,
        ResetZoom,
        ZoomIn,
        ZoomOut,
        ExpandActiveResizable,
        ContractActiveResizable,
        // Repository
        Push,
        Pull,
        Fetch,
        FetchAllRepositories,
        // Repository › Pull All Repositories (`299-pull-all-repositories`)
        PullAllRepositories,
        // Repository › Fetch All Tags (`899-tags-in-branch-list`)
        FetchAllTags,
        // Repository › Recent Activity… (`1216-recent-activity`)
        ShowRecentActivity,
        // Repository › Insights… (`1110-repository-insights`)
        ShowInsights,
        // Branch › Compare… (`1218-compare-refs`)
        CompareRefs,
        // Branch › Tags… (`1219-tag-manager`)
        ShowTags,
        // Branch › Check Out from Fork… (`1223-checkout-from-fork`)
        CheckoutFromFork,
        // Repository › Clean Untracked Files… (`1105-clean-untracked-files`)
        CleanUntrackedFiles,
        // Repository › Apply Patch from File… / from Clipboard (`1106-apply-patch`)
        ApplyPatchFromFile,
        ApplyPatchFromClipboard,
        // Repository › Submodules… (`1111-submodules`)
        ShowSubmodules,
        // Repository › Sparse Checkout… (`1112-sparse-checkout`)
        ShowSparseCheckout,
        // Repository › Issues… / New Issue… (`345-issues`)
        ShowIssues,
        NewIssue,
        // Repository › Releases… / Create Release… (`346-releases`)
        ShowReleases,
        CreateRelease,
        // Repository › Actions… (`351-actions`)
        ShowActions,
        // Repository › Start Bisect / Stop Bisecting (`1212-bisect`)
        StartBisect,
        StopBisect,
        // Branch › Request Reviewers… (`336-request-reviewers`)
        RequestReviewers,
        // Branch › Push To ▸ / Fetch From ▸ the remote at that index of
        // `MenuLabelsEvent::remotes` (`1210-push-to-other-remote`)
        // Repository › Open in Editor ▸ (`524-open-repository-with-editor`),
        // by index into `Dispatcher::menu_editors`
        OpenInChosenEditor0,
        OpenInChosenEditor1,
        OpenInChosenEditor2,
        OpenInChosenEditor3,
        OpenInChosenEditor4,
        OpenInChosenEditor5,
        OpenInChosenEditor6,
        OpenInChosenEditor7,
        PushToRemote0,
        PushToRemote1,
        PushToRemote2,
        PushToRemote3,
        PushToRemote4,
        PushToRemote5,
        PushToRemote6,
        PushToRemote7,
        FetchFromRemote0,
        FetchFromRemote1,
        FetchFromRemote2,
        FetchFromRemote3,
        FetchFromRemote4,
        FetchFromRemote5,
        FetchFromRemote6,
        FetchFromRemote7,
        // Branch › Move Changes to Worktree… (`283-move-changes-to-worktree`)
        MoveChangesToWorktree,
        // Branch › Stash All Changes with Message… (`797-stash-list`)
        StashAllChangesWithMessage,
        RemoveRepository,
        // Android only (no GHD equivalent): Repository › Move to shared storage…
        MoveToSharedStorage,
        ViewOnGitHub,
        ViewUpstreamOnGitHub,
        OpenInShell,
        ShowInFinder,
        OpenInEditor,
        OpenWith,
        CreateIssue,
        AddLicense,
        RepositorySettings,
        // Edit › Undo Last Commit (`423-undo-commit-menu-item`)
        UndoLastCommit,
        // Branch
        NewBranch,
        RenameBranch,
        DeleteBranch,
        DiscardAllChanges,
        StashAllChanges,
        UpdateFromDefaultBranch,
        CompareToBranch,
        MergeIntoCurrentBranch,
        SquashAndMergeIntoCurrentBranch,
        RebaseCurrentBranch,
        CompareOnGitHub,
        ViewBranchOnGitHub,
        PreviewPullRequest,
        CreatePullRequest,
        // Branch › Review Pull Request… (`348-pull-request-review`)
        ReviewPullRequest,
        // Window
        Minimize,
        Zoom,
        CloseWindow,
        BringAllToFront,
        ShowMainWindow,
        // File › New Window (`429-multiple-windows`)
        NewWindow,
        // `430-repository-tabs`
        NextTab,
        PreviousTab,
        CloseTab,
        // Help
        ReportIssue,
        ContactSupport,
        ShowUserGuides,
        ShowReleaseNotes,
        // Help › Show Test Notifications (debug builds; GHD test menu "Show notification")
        ShowTestNotifications,
        ShowKeyboardShortcuts,
        ShowLogs,
        // In-app
        Commit,
        ToggleSection,
        CloseFoldout,
    ]
);
