use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git was not found on this machine")]
    GitNotFound,
    #[error("git {found} is too old; Corvene needs {required} or newer")]
    GitTooOld { found: String, required: String },
    #[error("could not run git: {0}")]
    Spawn(#[source] std::io::Error),
    /// Flag `876`: the command's working directory is missing or a file.
    #[error(
        "The repository folder {} is missing or not a folder. It may have been moved, renamed \
         or deleted, or be on a drive that is not connected.",
        .0.display()
    )]
    MissingWorkdir(PathBuf),
    #[error("git {args} failed with exit code {code:?}: {}", .stderr.trim_end())]
    Failed {
        args: String,
        code: Option<i32>,
        /// What git wrote (GHD's `terminalOutput`): stdout and stderr, the
        /// last 256 KiB, untrimmed (see `process.rs`).
        stderr: String,
    },
    #[error("git output was not valid UTF-8")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} is not a git repository")]
    NotARepository(PathBuf),
    #[error("could not open repository: {0}")]
    Open(#[from] Box<gix::Error>),
    #[error("{0}")]
    Gix(String),
    /// The command was stopped through its [`crate::CancelToken`].
    #[error("git {0} was cancelled")]
    Cancelled(String),
    /// Corvene: the command ran past its [`crate::GitCommand::timeout`] and
    /// was stopped.
    #[error(
        "git {args} did not finish within {seconds} seconds and was stopped. Files that are only in the cloud (iCloud Drive, OneDrive, Dropbox) can keep git waiting for their download."
    )]
    TimedOut { args: String, seconds: u64 },
    /// GHD `clone`'s `isClonePathSensitive` backstop: the destination is the
    /// home directory or a credentials / configuration folder in it.
    #[error(
        "The clone destination \"{}\" targets a sensitive system location. Cloning into this directory is not allowed.",
        .0.display()
    )]
    SensitiveClonePath(PathBuf),
    /// GHD 3.6.7 `pull` (desktop/desktop#22862): `git pull` hands the
    /// remote's name to its `git fetch` without a `--`, so a name that
    /// starts with `-` would be read as an option.
    #[error("Cannot pull from a remote whose name starts with '-'.")]
    DashRemoteName(String),
}

impl From<gix::Error> for GitError {
    fn from(err: gix::Error) -> Self {
        GitError::Open(Box::new(err))
    }
}

pub type Result<T> = std::result::Result<T, GitError>;

/// GHD `getRepositoryType`'s `unsafe` case: the directory named by git's
/// "fatal: detected dubious ownership in repository at '<path>'".
pub fn dubious_ownership_path(stderr: &str) -> Option<PathBuf> {
    const MARKER: &str = "detected dubious ownership in repository at '";
    let start = stderr.find(MARKER)? + MARKER.len();
    let rest = &stderr[start..];
    // the path is quoted up to the end of the line
    let line = rest.lines().next()?;
    let end = line.rfind('\'')?;
    Some(PathBuf::from(&line[..end]))
}

/// The branch and worktree of git's refusals to use a branch that another
/// worktree has checked out: `'main' is already used by worktree at '<path>'`
/// (checkout, git 2.42+; older: `is already checked out at`) and `cannot
/// delete branch 'x' used by worktree at '<path>'` (older: `checked out at`).
pub fn branch_in_other_worktree(stderr: &str) -> Option<(String, PathBuf)> {
    const MARKERS: [&str; 4] = [
        "' is already used by worktree at '",
        "' is already checked out at '",
        "' used by worktree at '",
        "' checked out at '",
    ];
    stderr.lines().find_map(|line| {
        let (at, marker) = MARKERS
            .iter()
            .find_map(|m| line.find(m).map(|at| (at, *m)))?;
        let branch_start = line[..at].rfind('\'')? + 1;
        let rest = &line[at + marker.len()..];
        let path_end = rest.rfind('\'')?;
        Some((
            line[branch_start..at].to_string(),
            PathBuf::from(&rest[..path_end]),
        ))
    })
}

/// git's "bad config line <n> in file <path>": the line number and file.
pub fn bad_config_line(stderr: &str) -> Option<(usize, String)> {
    const MARKER: &str = "bad config line ";
    stderr.lines().find_map(|line| {
        let rest = &line[line.find(MARKER)? + MARKER.len()..];
        let (number, file) = rest.split_once(" in file ")?;
        Some((number.trim().parse().ok()?, file.trim().to_string()))
    })
}

/// Corvene addition (flag `875`): a sentence for [`bad_config_line`] that
/// names the file and line (GHD shows git's words, or "not a git
/// repository" when adding such a repository).
pub fn explain_bad_config(stderr: &str) -> Option<String> {
    let (line, file) = bad_config_line(stderr)?;
    Some(format!(
        "Git cannot read line {line} of {file}. Fix or remove that line (a merge conflict left \
         in the file is a common cause), then try again."
    ))
}

impl GitError {
    /// [`branch_in_other_worktree`] of a failed git command.
    pub fn branch_in_other_worktree(&self) -> Option<(String, PathBuf)> {
        match self {
            GitError::Failed { stderr, .. } => branch_in_other_worktree(stderr),
            _ => None,
        }
    }

    /// The unsafe directory when git refused to run because of its owner.
    pub fn unsafe_repository_path(&self) -> Option<PathBuf> {
        match self {
            GitError::Failed { stderr, .. } => dubious_ownership_path(stderr),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dubious_ownership() {
        let stderr = "fatal: detected dubious ownership in repository at '/Users/o'brien/repo'\n\
                      To add an exception for this directory, call:\n\n\
                      \tgit config --global --add safe.directory '/Users/o'brien/repo'\n";
        assert_eq!(
            dubious_ownership_path(stderr),
            Some(PathBuf::from("/Users/o'brien/repo"))
        );
        assert_eq!(dubious_ownership_path("fatal: not a git repository"), None);
    }

    #[test]
    fn parses_bad_config_line() {
        assert_eq!(
            bad_config_line("fatal: bad config line 14 in file .git/config\n"),
            Some((14, ".git/config".to_string()))
        );
        assert_eq!(
            bad_config_line("fatal: bad config line 2 in file /tmp/a b/.gitmodules"),
            Some((2, "/tmp/a b/.gitmodules".to_string()))
        );
        assert_eq!(
            bad_config_line("fatal: bad config line 3 in blob abc"),
            None
        );
        assert!(
            explain_bad_config("fatal: bad config line 14 in file .git/config")
                .is_some_and(|text| text.starts_with("Git cannot read line 14 of .git/config."))
        );
    }

    #[test]
    fn parses_branch_in_other_worktree() {
        let cases = [
            "fatal: 'main' is already used by worktree at '/tmp/w 2'\n",
            "fatal: 'main' is already checked out at '/tmp/w 2'\n",
            "error: cannot delete branch 'main' used by worktree at '/tmp/w 2'\n",
            "error: Cannot delete branch 'main' checked out at '/tmp/w 2'\n",
        ];
        for stderr in cases {
            assert_eq!(
                branch_in_other_worktree(stderr),
                Some(("main".to_string(), PathBuf::from("/tmp/w 2"))),
                "{stderr}"
            );
        }
        assert_eq!(
            branch_in_other_worktree("error: branch 'x' not found."),
            None
        );
    }
}
