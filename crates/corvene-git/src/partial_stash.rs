//! Corvene (`1303-partial-stash`): stash some lines of the changed files.
//!
//! A patch-based stash, never `git stash push -p` (interactive): the stash
//! commits are built in a temporary index, stored with `git stash store`,
//! and only then are the stashed lines taken out of the working copy with
//! the patch GHD's Discard uses (`formatPatchToDiscardChanges`). So a
//! failure before the store leaves everything as it was, and one after it
//! leaves the changes in the stash list.
//!
//! Only modified files can be stashed line by line. A new, deleted,
//! renamed or type-changed file that has any line selected goes into the
//! stash whole: half of a new file would come back as an untracked file
//! `git stash apply` refuses to overwrite. GHD stashes all changes or none.
//!
//! A UTF-16 file's lines (`1306-utf16-diffs`) are no patch: its stashed
//! version goes into the temporary index as a blob and its working copy is
//! rewritten ([`crate::utf16`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvene_models::{Diff, DiffSelectionType, FileStatusKind, WorkingDirectoryFileChange};

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::patch::{PatchOptions, format_patch_to_discard_changes_with, format_patch_with};
use crate::process::GitCommand;

/// One file's share of a partial stash.
enum Part<'a> {
    /// The whole file (any status).
    Whole(&'a WorkingDirectoryFileChange),
    /// Some lines of a modified file: the patch that adds them to `HEAD`'s
    /// version and the one that takes them out of the working copy.
    Lines { stash: Vec<u8>, discard: Vec<u8> },
    /// Some lines of a modified UTF-16 file (`1306-utf16-diffs`): `HEAD`'s
    /// version with them and the working copy without them, encoded.
    Utf16 {
        path: &'a str,
        stash: Vec<u8>,
        discard: Vec<u8>,
    },
}

/// Removes the temporary index whatever happens.
struct TempIndex(PathBuf);

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stash the selected changes of `files` (each file's `selection`; files
/// with nothing selected are skipped) as one entry with `message` (git's
/// list shows `On <branch>: <message>`), then take them out of the working
/// copy. Returns false when nothing was selected. `guard_assume_unchanged`
/// is flag `869`'s check.
pub fn create_partial_stash(
    git: Arc<GitBinary>,
    workdir: &Path,
    branch: &str,
    message: &str,
    files: &[WorkingDirectoryFileChange],
    options: PatchOptions,
    guard_assume_unchanged: bool,
) -> Result<bool> {
    let head = GitCommand::new(git.clone())
        .args(["rev-parse", "--verify", "-q", "HEAD^{commit}"])
        .current_dir(workdir)
        .allow_exit_code(1)
        .run()?
        .stdout_string()?
        .trim()
        .to_string();
    if head.is_empty() {
        return Err(GitError::Gix(
            "Changes can be stashed once the repository has a first commit.".into(),
        ));
    }
    let mut parts = Vec::new();
    for file in files {
        if let Some(part) = part_of(git.clone(), workdir, file, options)? {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Ok(false);
    }
    let whole: Vec<&WorkingDirectoryFileChange> = parts
        .iter()
        .filter_map(|p| match p {
            Part::Whole(f) => Some(*f),
            Part::Lines { .. } | Part::Utf16 { .. } => None,
        })
        .collect();
    let whole_paths: Vec<String> = whole
        .iter()
        .flat_map(|f| std::iter::once(f.path.clone()).chain(f.old_path.clone()))
        .collect();
    let (stash_patch, discard_patch) = parts.iter().fold(
        (Vec::new(), Vec::new()),
        |(mut stash, mut discard), part| {
            if let Part::Lines {
                stash: s,
                discard: d,
            } = part
            {
                stash.extend_from_slice(s);
                discard.extend_from_slice(d);
            }
            (stash, discard)
        },
    );
    if guard_assume_unchanged {
        let touched: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let hidden: Vec<String> = crate::modified_assume_unchanged(git.clone(), workdir)?
            .into_iter()
            .filter(|p| touched.contains(p.as_str()))
            .collect();
        if !hidden.is_empty() {
            return Err(GitError::Gix(format!(
                "Your changes were not stashed: {} {} marked assume-unchanged. Run git \
                 update-index --no-assume-unchanged first.",
                hidden.join(", "),
                if hidden.len() == 1 { "is" } else { "are" },
            )));
        }
    }

    // 1. the stash's trees, in a temporary index; nothing else is touched
    let index = TempIndex(crate::paths::git_dir(workdir).join(format!(
        "corvene-partial-stash-{}.index",
        std::process::id()
    )));
    let in_temp = |args: &[&str], stdin: Option<Vec<u8>>| -> Result<String> {
        let mut cmd = GitCommand::new(git.clone())
            .args(args)
            .env("GIT_INDEX_FILE", &index.0)
            .current_dir(workdir);
        if let Some(stdin) = stdin {
            cmd = cmd.stdin(stdin);
        }
        Ok(cmd.run()?.stdout_string()?.trim().to_string())
    };
    in_temp(&["read-tree", &head], None)?;
    // Desktop stashes keep new files in the index commit too, which is how
    // a restore knows to leave them untracked (`775`)
    let new_files: Vec<String> = whole
        .iter()
        .filter(|f| {
            matches!(
                f.status.kind,
                FileStatusKind::New | FileStatusKind::Untracked
            )
        })
        .map(|f| f.path.clone())
        .collect();
    if !new_files.is_empty() {
        in_temp(
            &["update-index", "--add", "-z", "--stdin"],
            Some(nul_separated(&new_files)),
        )?;
    }
    let index_tree = in_temp(&["write-tree"], None)?;
    if !whole_paths.is_empty() {
        in_temp(
            &["update-index", "--add", "--remove", "-z", "--stdin"],
            Some(nul_separated(&whole_paths)),
        )?;
    }
    for part in &parts {
        if let Part::Utf16 { path, stash, .. } = part {
            crate::utf16::add_to_index(git.clone(), workdir, path, stash.clone(), Some(&index.0))?;
        }
    }
    if !stash_patch.is_empty() {
        in_temp(
            &[
                "apply",
                "--cached",
                "--unidiff-zero",
                "--whitespace=nowarn",
                "-",
            ],
            Some(stash_patch),
        )?;
    }
    let work_tree = in_temp(&["write-tree"], None)?;
    let head_tree = GitCommand::new(git.clone())
        .args(["rev-parse", &format!("{head}^{{tree}}")])
        .current_dir(workdir)
        .run()?
        .stdout_string()?;
    if work_tree == head_tree.trim() {
        return Ok(false);
    }
    // the lines must come out of the working copy cleanly before anything
    // is stored
    if !discard_patch.is_empty() {
        GitCommand::new(git.clone())
            .args([
                "apply",
                "--check",
                "--unidiff-zero",
                "--whitespace=nowarn",
                "-",
            ])
            .current_dir(workdir)
            .stdin(discard_patch.clone())
            .run()?;
    }

    // 2. the stash commits, the same shape `git stash push` makes
    let subject = GitCommand::new(git.clone())
        .args(["log", "-1", "--format=%h %s", &head])
        .current_dir(workdir)
        .run()?
        .stdout_string()?
        .trim()
        .to_string();
    let commit_tree = |tree: &str, message: &str, parents: &[&str]| -> Result<String> {
        let mut args = vec!["commit-tree", tree, "-m", message];
        for parent in parents {
            args.extend(["-p", parent]);
        }
        Ok(GitCommand::new(git.clone())
            .args(args)
            .current_dir(workdir)
            .run()?
            .stdout_string()?
            .trim()
            .to_string())
    };
    let index_commit = commit_tree(
        &index_tree,
        &format!("index on {branch}: {subject}"),
        &[&head],
    )?;
    let reflog_message = format!("On {branch}: {message}");
    let stash = commit_tree(&work_tree, &reflog_message, &[&head, &index_commit])?;
    GitCommand::new(git.clone())
        .args(["stash", "store", "-m", &reflog_message, &stash])
        .current_dir(workdir)
        .run()?;

    // 3. take the stashed changes out of the working copy
    let rewritten: Vec<(&str, &[u8])> = parts
        .iter()
        .filter_map(|part| match part {
            Part::Utf16 { path, discard, .. } => Some((*path, discard.as_slice())),
            _ => None,
        })
        .collect();
    remove_from_working_copy(
        git,
        workdir,
        &head,
        &whole_paths,
        &discard_patch,
        &rewritten,
    )
    .map_err(|err| {
        GitError::Gix(format!(
            "The changes were stashed, but not all of them could be taken out of your files; \
             the stash list has them. {err}"
        ))
    })?;
    Ok(true)
}

/// What of `file` goes into the stash (`None`: nothing selected).
fn part_of<'a>(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &'a WorkingDirectoryFileChange,
    options: PatchOptions,
) -> Result<Option<Part<'a>>> {
    if file.status.is_conflicted() {
        return Err(GitError::Gix(format!(
            "{} has conflicts; resolve them before stashing it.",
            file.path
        )));
    }
    if file.status.submodule {
        return Err(GitError::Gix(format!(
            "{} is a submodule; its changes cannot be stashed this way.",
            file.path
        )));
    }
    if file.path.ends_with('/') {
        return Err(GitError::Gix(format!(
            "{} is a repository of its own and cannot be stashed.",
            file.path
        )));
    }
    match file.selection.kind() {
        DiffSelectionType::None => return Ok(None),
        DiffSelectionType::All => return Ok(Some(Part::Whole(file))),
        DiffSelectionType::Partial if file.status.kind != FileStatusKind::Modified => {
            return Ok(Some(Part::Whole(file)));
        }
        DiffSelectionType::Partial => {}
    }
    // the diff the line selection was made in (`stage_partial_files_with`)
    let diff =
        crate::diff::working_directory_diff(git.clone(), workdir, file, false, false, false, None)?;
    // `1331-notebook-diff` / `1332-latex-diff`: the lines of git's own diff
    let diff = match diff {
        Diff::Notebook { source, .. } | Diff::Latex { source, .. } => *source,
        diff => diff,
    };
    let (hunks, utf16) = match &diff {
        Diff::Text { hunks, warnings } | Diff::LargeText { hunks, warnings } => {
            (hunks, warnings.utf16.as_ref())
        }
        Diff::Empty => return Ok(None),
        _ => return Ok(Some(Part::Whole(file))),
    };
    if let Some(utf16) = utf16 {
        let selected =
            crate::utf16::apply_selection(git, workdir, &file.path, hunks, &file.selection, utf16)?;
        return Ok(selected.map(|selected| Part::Utf16 {
            path: &file.path,
            stash: selected.with,
            discard: selected.without,
        }));
    }
    let stash = format_patch_with(file, hunks, options);
    let discard = format_patch_to_discard_changes_with(&file.path, hunks, &file.selection, options);
    Ok(match (stash, discard) {
        (Some(stash), Some(discard)) => Some(Part::Lines { stash, discard }),
        _ => None,
    })
}

/// After the store: the selected lines out of their files (`discard`, and
/// the UTF-16 files `rewritten` with what is left), the whole files back to
/// `HEAD` (index and working copy), and those `HEAD` lacks deleted.
fn remove_from_working_copy(
    git: Arc<GitBinary>,
    workdir: &Path,
    head: &str,
    whole_paths: &[String],
    discard: &[u8],
    rewritten: &[(&str, &[u8])],
) -> Result<()> {
    if !discard.is_empty() {
        crate::patch::discard_changes_from_selection(git.clone(), workdir, discard)?;
    }
    for (path, bytes) in rewritten {
        std::fs::write(workdir.join(path), bytes)?;
    }
    if whole_paths.is_empty() {
        return Ok(());
    }
    let listed = GitCommand::new(git.clone())
        .args(["ls-tree", "-r", "-z", "--name-only", head, "--"])
        .args(whole_paths)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(workdir)
        .run()?;
    let in_head: HashSet<String> = listed
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    let (tracked, added): (Vec<String>, Vec<String>) = whole_paths
        .iter()
        .cloned()
        .partition(|p| in_head.contains(p));
    if !tracked.is_empty() {
        GitCommand::new(git.clone())
            .args([
                "checkout",
                head,
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ])
            .env("GIT_LITERAL_PATHSPECS", "1")
            .current_dir(workdir)
            .stdin(nul_separated(&tracked))
            .run()?;
    }
    if !added.is_empty() {
        GitCommand::new(git)
            .args(["update-index", "--force-remove", "-z", "--stdin"])
            .current_dir(workdir)
            .stdin(nul_separated(&added))
            .run()?;
        for path in &added {
            let full = workdir.join(path);
            match std::fs::remove_file(&full) {
                Ok(()) => remove_empty_parents(workdir, &full),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
    Ok(())
}

/// The folders `file` leaves empty, up to (not including) `workdir`, as
/// `git stash push` removes them.
fn remove_empty_parents(workdir: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == workdir || !d.starts_with(workdir) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

fn nul_separated(paths: &[String]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for path in paths {
        out.extend_from_slice(path.as_bytes());
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvene_models::DiffSelection;
    use std::process::Command;

    fn git() -> Arc<GitBinary> {
        Arc::new(crate::find_git().expect("git"))
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        run(p, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            // Git for Windows checks text out with CRLF unless told not to
            ("core.autocrlf", "false"),
            ("user.name", "T"),
            ("user.email", "t@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            run(p, &["config", k, v]);
        }
        std::fs::write(p.join("a.txt"), "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n").expect("write");
        std::fs::write(p.join("b.txt"), "b\n").expect("write");
        run(p, &["add", "-A"]);
        run(p, &["commit", "-q", "-m", "one"]);
        dir
    }

    fn changes(dir: &Path) -> Vec<WorkingDirectoryFileChange> {
        crate::get_status(git(), dir).expect("status").files
    }

    #[test]
    fn stashes_lines_of_a_utf16_file() {
        crate::utf16::set_decode_utf16(true);
        let dir = repo();
        let p = dir.path();
        let format = corvene_models::Utf16Format {
            big_endian: false,
            bom: true,
        };
        let encode = |text: &str| crate::utf16::encode(text, format);
        let head = "1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8\r\n9\r\n10\r\n";
        std::fs::write(p.join("ea.mq5"), encode(head)).expect("write");
        run(p, &["add", "-A"]);
        run(p, &["commit", "-q", "-m", "utf16"]);
        let edited = head
            .replace("1\r\n2", "ONE\r\n2")
            .replace("10\r\n", "TEN\r\n");
        std::fs::write(p.join("ea.mq5"), encode(&edited)).expect("write");
        let mut files = changes(p);
        // lines: 0 @@ 1 -1 2 +ONE ...; the first change only
        files[0].selection = DiffSelection::none().with_range(1, 2, true);
        assert!(
            create_partial_stash(
                git(),
                p,
                "main",
                "utf16 lines",
                &files,
                PatchOptions::default(),
                true,
            )
            .expect("stash")
        );
        assert_eq!(
            std::fs::read(p.join("ea.mq5")).expect("read"),
            encode(&head.replace("10\r\n", "TEN\r\n"))
        );
        let stashed = Command::new("git")
            .args(["show", "stash@{0}:ea.mq5"])
            .current_dir(p)
            .output()
            .expect("git");
        assert_eq!(stashed.stdout, encode(&head.replace("1\r\n2", "ONE\r\n2")));
    }

    #[test]
    fn stashes_only_the_selected_lines() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("a.txt"), "ONE\n2\n3\n4\n5\n6\n7\n8\n9\nTEN\n").expect("write");
        std::fs::write(p.join("new.txt"), "new\n").expect("write");
        let mut files = changes(p);
        // lines: 0 @@ 1 -1 2 +ONE ... second hunk: -10 +TEN; take the first
        // hunk of a.txt only, plus new.txt whole; b.txt is unchanged
        for f in &mut files {
            f.selection = match f.path.as_str() {
                "a.txt" => DiffSelection::none().with_range(1, 2, true),
                _ => DiffSelection::all(),
            };
        }
        assert!(
            create_partial_stash(
                git(),
                p,
                "main",
                "some lines",
                &files,
                PatchOptions::default(),
                true,
            )
            .expect("stash")
        );
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).expect("read"),
            "1\n2\n3\n4\n5\n6\n7\n8\n9\nTEN\n"
        );
        assert!(!p.join("new.txt").exists());
        assert_eq!(
            run(p, &["stash", "list", "--format=%gs"]).trim(),
            "On main: some lines"
        );
        let stashed = run(p, &["show", "stash@{0}:a.txt"]);
        assert_eq!(stashed, "ONE\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        assert_eq!(run(p, &["show", "stash@{0}:new.txt"]), "new\n");
        // index commit carries the new file (Desktop's way)
        assert_eq!(run(p, &["show", "stash@{0}^2:new.txt"]), "new\n");
        // git refuses to pop it over the change left in a.txt; merged in,
        // it comes back on top of it
        let entry = crate::get_stashes(git(), p).expect("stashes").0[0].clone();
        let options = crate::StashPopOptions {
            merge_over_local_changes: true,
            ..Default::default()
        };
        assert_eq!(
            crate::pop_stash_entry_with(git(), p, &entry.sha, options).expect("pop"),
            crate::StashPop::Restored
        );
        assert_eq!(run(p, &["stash", "list"]).trim(), "");
        assert_eq!(
            std::fs::read_to_string(p.join("a.txt")).expect("read"),
            "ONE\n2\n3\n4\n5\n6\n7\n8\n9\nTEN\n"
        );
        assert!(p.join("new.txt").exists());
    }

    #[test]
    fn whole_deleted_file_comes_back() {
        let dir = repo();
        let p = dir.path();
        std::fs::remove_file(p.join("b.txt")).expect("rm");
        let files = changes(p);
        assert!(
            create_partial_stash(
                git(),
                p,
                "main",
                "m",
                &files,
                PatchOptions::default(),
                false
            )
            .expect("stash")
        );
        assert_eq!(
            std::fs::read_to_string(p.join("b.txt")).expect("read"),
            "b\n"
        );
        assert!(changes(p).is_empty());
    }

    #[test]
    fn nothing_selected_stashes_nothing() {
        let dir = repo();
        let p = dir.path();
        std::fs::write(p.join("b.txt"), "B\n").expect("write");
        let mut files = changes(p);
        files[0].selection = DiffSelection::none();
        assert!(
            !create_partial_stash(
                git(),
                p,
                "main",
                "m",
                &files,
                PatchOptions::default(),
                false
            )
            .expect("stash")
        );
        assert_eq!(run(p, &["stash", "list"]).trim(), "");
    }
}
