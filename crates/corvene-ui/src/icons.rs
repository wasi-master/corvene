//! Primer Octicons (16 px) bundled in `assets/octicons/`.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt, AnyElement, ElementId, Hsla, IntoElement, Styled, Svg, Transformation,
    percentage, svg,
};

use crate::theme::sizes::ICON_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Octicon {
    Repo,
    RepoForked,
    RepoClone,
    RepoPush,
    RepoPull,
    GitBranch,
    GitCommit,
    GitMerge,
    GitCompare,
    GitPullRequest,
    Sync,
    /// GHD `syncClockwise`: `sync` flipped horizontally, used by `Loading`.
    SyncClockwise,
    ArrowUp,
    ArrowDown,
    /// GHD `arrowRight`: a rename's `.rename-arrow`.
    ArrowRight,
    Upload,
    TriangleDown,
    TriangleRight,
    ChevronDown,
    ChevronRight,
    Filter,
    Search,
    Check,
    CheckCircleFill,
    XCircleFill,
    DiffAdded,
    DiffModified,
    DiffRemoved,
    DiffRenamed,
    DiffIgnored,
    FileDiff,
    Gear,
    PersonAdd,
    X,
    Plus,
    FileDirectory,
    Lock,
    Verified,
    Unverified,
    Shield,
    ShieldX,
    ShieldCheck,
    DeviceDesktop,
    Copy,
    Fold,
    FoldUp,
    FoldDown,
    Unfold,
    DotFill,
    KebabHorizontal,
    Question,
    Bell,
    Paintbrush,
    Plug,
    Home,
    Accessibility,
    Tag,
    History,
    Alert,
    Info,
    IssueOpened,
    IssueClosed,
    Comment,
    Package,
    LinkExternal,
    Terminal,
    FileCode,
    Pencil,
    Trash,
    Undo,
    CloudOffline,
    Eye,
    EyeClosed,
    DesktopDownload,
    FileSymlinkFile,
    FileSubmodule,
    FileBinary,
    Image,
    /// GHD custom 12 px check/dash for the diff gutter (`ui/octicons/diff.ts`).
    DiffCheck,
    DiffDash,
    /// GHD custom stash icon (`filter-changes-list.tsx` `StashIcon`).
    Stash,
    Server,
    File,
    Person,
    GitPullRequestDraft,
    Stop,
    Skip,
    SquareFill,
    IssueReopened,
    ChevronUp,
    ListUnordered,
    MortarBoard,
    Telescope,
    Zap,
    LightBulb,
    Stack,
    /// `351-actions`
    Play,
    Workflow,
    Clock,
    Stopwatch,
    /// `1310-file-list-tree` folder rows.
    FileDirectoryFill,
    FileDirectoryOpenFill,
    /// `117-file-icons` built-in set.
    Book,
    Container,
    Database,
    FileZip,
    Law,
    Markdown,
    /// `1335-video-diff` / `1336-audio-diff` transport glyphs (drawn for
    /// Corvene in the octicon style; not part of Primer).
    MediaPlay,
    MediaPause,
    MediaFrameBack,
    MediaFrameForward,
    MediaPrevChange,
    MediaNextChange,
    MediaVolume,
    MediaMute,
    MediaLoop,
}

impl Octicon {
    pub fn path(self) -> &'static str {
        match self {
            Octicon::Repo => "octicons/repo-16.svg",
            Octicon::RepoForked => "octicons/repo-forked-16.svg",
            Octicon::RepoClone => "octicons/repo-clone-16.svg",
            Octicon::RepoPush => "octicons/repo-push-16.svg",
            Octicon::RepoPull => "octicons/repo-pull-16.svg",
            Octicon::GitBranch => "octicons/git-branch-16.svg",
            Octicon::GitCommit => "octicons/git-commit-16.svg",
            Octicon::GitMerge => "octicons/git-merge-16.svg",
            Octicon::GitCompare => "octicons/git-compare-16.svg",
            Octicon::GitPullRequest => "octicons/git-pull-request-16.svg",
            Octicon::Sync => "octicons/sync-16.svg",
            Octicon::SyncClockwise => "octicons/sync-clockwise-16.svg",
            Octicon::ArrowUp => "octicons/arrow-up-16.svg",
            Octicon::ArrowDown => "octicons/arrow-down-16.svg",
            Octicon::ArrowRight => "octicons/arrow-right-16.svg",
            Octicon::Upload => "octicons/upload-16.svg",
            Octicon::TriangleDown => "octicons/triangle-down-16.svg",
            Octicon::TriangleRight => "octicons/triangle-right-16.svg",
            Octicon::ChevronDown => "octicons/chevron-down-16.svg",
            Octicon::ChevronRight => "octicons/chevron-right-16.svg",
            Octicon::Filter => "octicons/filter-16.svg",
            Octicon::Search => "octicons/search-16.svg",
            Octicon::Check => "octicons/check-16.svg",
            Octicon::CheckCircleFill => "octicons/check-circle-fill-16.svg",
            Octicon::XCircleFill => "octicons/x-circle-fill-16.svg",
            Octicon::DiffAdded => "octicons/diff-added-16.svg",
            Octicon::DiffModified => "octicons/diff-modified-16.svg",
            Octicon::DiffRemoved => "octicons/diff-removed-16.svg",
            Octicon::DiffRenamed => "octicons/diff-renamed-16.svg",
            Octicon::DiffIgnored => "octicons/diff-ignored-16.svg",
            Octicon::FileDiff => "octicons/file-diff-16.svg",
            Octicon::Gear => "octicons/gear-16.svg",
            Octicon::PersonAdd => "octicons/person-add-16.svg",
            Octicon::X => "octicons/x-16.svg",
            Octicon::Plus => "octicons/plus-16.svg",
            Octicon::FileDirectory => "octicons/file-directory-16.svg",
            Octicon::Lock => "octicons/lock-16.svg",
            Octicon::Verified => "octicons/verified-16.svg",
            Octicon::Unverified => "octicons/unverified-16.svg",
            Octicon::Shield => "octicons/shield-16.svg",
            Octicon::ShieldX => "octicons/shield-x-16.svg",
            Octicon::ShieldCheck => "octicons/shield-check-16.svg",
            Octicon::DeviceDesktop => "octicons/device-desktop-16.svg",
            Octicon::Copy => "octicons/copy-16.svg",
            Octicon::Fold => "octicons/fold-16.svg",
            Octicon::FoldUp => "octicons/fold-up-16.svg",
            Octicon::FoldDown => "octicons/fold-down-16.svg",
            Octicon::Unfold => "octicons/unfold-16.svg",
            Octicon::DotFill => "octicons/dot-fill-16.svg",
            Octicon::KebabHorizontal => "octicons/kebab-horizontal-16.svg",
            Octicon::Question => "octicons/question-16.svg",
            Octicon::Bell => "octicons/bell-16.svg",
            Octicon::Paintbrush => "octicons/paintbrush-16.svg",
            Octicon::Plug => "octicons/plug-16.svg",
            Octicon::Home => "octicons/home-16.svg",
            Octicon::Accessibility => "octicons/accessibility-16.svg",
            Octicon::Tag => "octicons/tag-16.svg",
            Octicon::History => "octicons/history-16.svg",
            Octicon::Alert => "octicons/alert-16.svg",
            Octicon::DiffCheck => "octicons/diff-check-12.svg",
            Octicon::DiffDash => "octicons/diff-dash-12.svg",
            Octicon::Stash => "octicons/stash-16.svg",
            Octicon::Server => "octicons/server-16.svg",
            Octicon::File => "octicons/file-16.svg",
            Octicon::Person => "octicons/person-16.svg",
            Octicon::GitPullRequestDraft => "octicons/git-pull-request-draft-16.svg",
            Octicon::Stop => "octicons/stop-16.svg",
            Octicon::Skip => "octicons/skip-16.svg",
            Octicon::SquareFill => "octicons/square-fill-16.svg",
            Octicon::IssueReopened => "octicons/issue-reopened-16.svg",
            Octicon::ChevronUp => "octicons/chevron-up-16.svg",
            Octicon::Info => "octicons/info-16.svg",
            Octicon::IssueOpened => "octicons/issue-opened-16.svg",
            Octicon::IssueClosed => "octicons/issue-closed-16.svg",
            Octicon::Comment => "octicons/comment-16.svg",
            Octicon::Package => "octicons/package-16.svg",
            Octicon::LinkExternal => "octicons/link-external-16.svg",
            Octicon::ListUnordered => "octicons/list-unordered-16.svg",
            Octicon::MortarBoard => "octicons/mortar-board-16.svg",
            Octicon::LightBulb => "octicons/light-bulb-16.svg",
            Octicon::Stack => "octicons/stack-16.svg",
            Octicon::Play => "octicons/play-16.svg",
            Octicon::Workflow => "octicons/workflow-16.svg",
            Octicon::Clock => "octicons/clock-16.svg",
            Octicon::Stopwatch => "octicons/stopwatch-16.svg",
            Octicon::FileDirectoryFill => "octicons/file-directory-fill-16.svg",
            Octicon::FileDirectoryOpenFill => "octicons/file-directory-open-fill-16.svg",
            Octicon::Book => "octicons/book-16.svg",
            Octicon::Container => "octicons/container-16.svg",
            Octicon::Database => "octicons/database-16.svg",
            Octicon::FileZip => "octicons/file-zip-16.svg",
            Octicon::Law => "octicons/law-16.svg",
            Octicon::Markdown => "octicons/markdown-16.svg",
            Octicon::Telescope => "octicons/telescope-16.svg",
            Octicon::Zap => "octicons/zap-16.svg",
            Octicon::Terminal => "octicons/terminal-16.svg",
            Octicon::FileCode => "octicons/file-code-16.svg",
            Octicon::Pencil => "octicons/pencil-16.svg",
            Octicon::Trash => "octicons/trash-16.svg",
            Octicon::Undo => "octicons/undo-16.svg",
            Octicon::CloudOffline => "octicons/cloud-offline-16.svg",
            Octicon::Eye => "octicons/eye-16.svg",
            Octicon::EyeClosed => "octicons/eye-closed-16.svg",
            Octicon::DesktopDownload => "octicons/desktop-download-16.svg",
            Octicon::FileSymlinkFile => "octicons/file-symlink-file-16.svg",
            Octicon::FileSubmodule => "octicons/file-submodule-16.svg",
            Octicon::FileBinary => "octicons/file-binary-16.svg",
            Octicon::Image => "octicons/image-16.svg",
            Octicon::MediaPlay => "octicons/media-play-16.svg",
            Octicon::MediaPause => "octicons/media-pause-16.svg",
            Octicon::MediaFrameBack => "octicons/media-frame-back-16.svg",
            Octicon::MediaFrameForward => "octicons/media-frame-forward-16.svg",
            Octicon::MediaPrevChange => "octicons/media-prev-change-16.svg",
            Octicon::MediaNextChange => "octicons/media-next-change-16.svg",
            Octicon::MediaVolume => "octicons/media-volume-16.svg",
            Octicon::MediaMute => "octicons/media-mute-16.svg",
            Octicon::MediaLoop => "octicons/media-loop-16.svg",
        }
    }
}

/// A 16 px octicon filled with `color` (`svg.octicon { fill: currentColor }`).
/// GPUI's `svg()` does not inherit the parent's text colour, so it is explicit.
pub fn octicon(icon: Octicon, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .size(ICON_SIZE())
        .flex_none()
        .text_color(color)
}

/// Spins `icon` like GHD's `.spin` class (`animation: spin 1s linear
/// infinite`, `ui/toolbar/_toolbar.scss`). `id` keys the animation state and
/// must be unique among its siblings.
pub fn spin(icon: Svg, id: impl Into<ElementId>) -> AnyElement {
    icon.with_animation(
        id,
        Animation::new(Duration::from_secs(1)).repeat(),
        |icon, delta| icon.with_transformation(Transformation::rotate(percentage(delta))),
    )
    .into_any_element()
}

/// GHD `Loading` (`ui/lib/loading.tsx`): a spinning `syncClockwise`.
pub fn loading(id: impl Into<ElementId>, color: Hsla) -> AnyElement {
    spin(octicon(Octicon::SyncClockwise, color), id)
}

/// GHD `Repository | CloningRepository`, what [`icon_for_repository`]
/// takes.
#[derive(Clone, Copy)]
pub enum RepositoryOrCloning<'a> {
    Repository(&'a corvene_core::Repository),
    /// A clone in progress (GHD `CloningRepository`).
    Cloning(&'a corvene_core::CloneState),
}

/// GHD `iconForRepository` (`ui/octicons/repository.ts`): download for a
/// clone in progress, alert for a missing repository, computer without a
/// GitHub repository, lock when private, fork for a fork, else repo.
pub fn icon_for_repository(repository: RepositoryOrCloning<'_>) -> Octicon {
    let repository = match repository {
        RepositoryOrCloning::Cloning(_) => return Octicon::DesktopDownload,
        RepositoryOrCloning::Repository(r) => r,
    };
    if repository.missing {
        return Octicon::Alert;
    }
    match &repository.github {
        None => Octicon::DeviceDesktop,
        Some(gh) if gh.private => Octicon::Lock,
        Some(gh) if gh.fork => Octicon::RepoForked,
        Some(_) => Octicon::Repo,
    }
}
