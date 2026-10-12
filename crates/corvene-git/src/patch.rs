//! Partial staging - GHD `lib/patch-formatter.ts` (`formatPatch`) and
//! `lib/git/apply.ts` (`applyPatchToIndex`).
//!
//! Deviation ([`PatchOptions`]): with `788-partial-commit-hunk-positions`
//! a hunk's start on the produced side counts only the hunks the patch
//! writes, not the changes left out of it; with `789-non-utf8-diffs` a
//! line that is not UTF-8 is written as its original bytes.
//!
//! A UTF-16 file's selection (`1306-utf16-diffs`, a diff marked
//! `DiffWarnings::utf16`) is not a patch: [`crate::utf16`] writes the
//! index blob.

use std::path::Path;
use std::sync::Arc;

use corvene_models::{
    Diff, DiffHunk, DiffLine, DiffLineKind, DiffSelectionType, FileStatusKind,
    WorkingDirectoryFileChange,
};

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::process::GitCommand;

/// Corvene deviations in the partial patches; the default is GHD's patch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatchOptions {
    /// `788-partial-commit-hunk-positions`: each hunk's start on the side
    /// the patch produces counts only the hunks written before it. GHD
    /// copies the start from the full diff, which also counts the changes
    /// left out, so `git apply` (which starts looking for a hunk there)
    /// can match its context at a later, identical spot.
    pub exact_hunk_starts: bool,
    /// `789-non-utf8-diffs`: a line that is not UTF-8 is written as its
    /// bytes (`DiffLine::raw`); GHD writes the replacement characters it
    /// decoded, which no longer match the file, so `git apply` refuses.
    pub raw_lines: bool,
}

fn format_patch_header(from: Option<&str>, to: Option<&str>) -> String {
    let from = from.map(|p| format!("a/{p}")).unwrap_or("/dev/null".into());
    let to = to.map(|p| format!("b/{p}")).unwrap_or("/dev/null".into());
    format!("--- {from}\n+++ {to}\n")
}

fn format_hunk_header(old_start: u32, old_count: u32, new_start: u32, new_count: u32) -> String {
    let before = if old_count == 1 {
        old_start.to_string()
    } else {
        format!("{old_start},{old_count}")
    };
    let after = if new_count == 1 {
        new_start.to_string()
    } else {
        format!("{new_start},{new_count}")
    };
    format!("@@ -{before} +{after} @@\n")
}

/// Where a hunk starts on the side a patch produces, when the hunk starts at
/// `start` (with `count` lines) on the side it applies to and the hunks
/// before it changed the line count by `delta`. As in git's own headers, a
/// side with no lines names the line before the hunk.
fn shifted_start(start: u32, count: u32, delta: i64, produced_count: u32) -> u32 {
    let unchanged_before = if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    };
    let before = u32::try_from((i64::from(unchanged_before) + delta).max(0)).unwrap_or(u32::MAX);
    if produced_count == 0 {
        before
    } else {
        before + 1
    }
}

fn push_line(buf: &mut Vec<u8>, marker: u8, line: &DiffLine, options: PatchOptions) {
    buf.push(marker);
    match line.raw.as_deref() {
        Some(raw) if options.raw_lines => buf.extend_from_slice(raw),
        _ => buf.extend_from_slice(line.text.as_bytes()),
    }
    buf.push(b'\n');
}

/// Build a unified diff containing only the selected additions/deletions of
/// `file`, suitable for `git apply --cached --unidiff-zero`. Returns `None`
/// when nothing is selected. GHD's patch ([`format_patch_with`]).
pub fn format_patch(file: &WorkingDirectoryFileChange, hunks: &[DiffHunk]) -> Option<String> {
    format_patch_with(file, hunks, PatchOptions::default())
        .map(|patch| String::from_utf8_lossy(&patch).into_owned())
}

/// [`format_patch`] with Corvene's [`PatchOptions`].
pub fn format_patch_with(
    file: &WorkingDirectoryFileChange,
    hunks: &[DiffHunk],
    options: PatchOptions,
) -> Option<Vec<u8>> {
    let is_new = matches!(
        file.status.kind,
        FileStatusKind::New | FileStatusKind::Untracked
    );
    let mut patch: Vec<u8> = Vec::new();
    let mut delta = 0i64;
    for hunk in hunks {
        let mut buf: Vec<u8> = Vec::new();
        let mut old_count = 0u32;
        let mut new_count = 0u32;
        let mut any_change = false;
        for (index, line) in hunk.lines.iter().enumerate() {
            let absolute = hunk.unified_diff_start + index as u32;
            match line.kind {
                DiffLineKind::Hunk => continue,
                DiffLineKind::Context => {
                    push_line(&mut buf, b' ', line, options);
                    old_count += 1;
                    new_count += 1;
                }
                DiffLineKind::Add | DiffLineKind::Delete
                    if file.selection.is_selected(absolute) =>
                {
                    if line.kind == DiffLineKind::Add {
                        push_line(&mut buf, b'+', line, options);
                        new_count += 1;
                    } else {
                        push_line(&mut buf, b'-', line, options);
                        old_count += 1;
                    }
                    any_change = true;
                }
                // Unselected lines in a new file never existed as far as
                // this patch is concerned; an unselected addition is dropped.
                DiffLineKind::Add => continue,
                _ if is_new => continue,
                // An unselected deletion stays in the file: context line.
                DiffLineKind::Delete => {
                    push_line(&mut buf, b' ', line, options);
                    old_count += 1;
                    new_count += 1;
                }
            }
            if line.no_trailing_newline {
                buf.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
        if !any_change {
            continue;
        }
        let new_start = if options.exact_hunk_starts {
            shifted_start(hunk.old_start, old_count, delta, new_count)
        } else {
            hunk.new_start
        };
        delta += i64::from(new_count) - i64::from(old_count);
        patch.extend_from_slice(
            format_hunk_header(hunk.old_start, old_count, new_start, new_count).as_bytes(),
        );
        patch.extend_from_slice(&buf);
    }
    if patch.is_empty() {
        return None;
    }
    let header = if is_new {
        format_patch_header(None, Some(&file.path))
    } else {
        format_patch_header(Some(&file.path), Some(&file.path))
    };
    let mut out = header.into_bytes();
    out.extend_from_slice(&patch);
    Some(out)
}

/// GHD `formatPatchToDiscardChanges`: a patch that undoes the selected lines
/// in the working copy (selected additions become deletions and vice versa,
/// everything else is context). `None` when nothing is selected. GHD's
/// patch ([`format_patch_to_discard_changes_with`]).
pub fn format_patch_to_discard_changes(
    path: &str,
    hunks: &[DiffHunk],
    selection: &corvene_models::DiffSelection,
) -> Option<String> {
    format_patch_to_discard_changes_with(path, hunks, selection, PatchOptions::default())
        .map(|patch| String::from_utf8_lossy(&patch).into_owned())
}

/// [`format_patch_to_discard_changes`] with Corvene's [`PatchOptions`].
pub fn format_patch_to_discard_changes_with(
    path: &str,
    hunks: &[DiffHunk],
    selection: &corvene_models::DiffSelection,
    options: PatchOptions,
) -> Option<Vec<u8>> {
    let mut patch: Vec<u8> = Vec::new();
    let mut delta = 0i64;
    for hunk in hunks {
        let mut buf: Vec<u8> = Vec::new();
        let mut old_count = 0u32;
        let mut new_count = 0u32;
        let mut any_change = false;
        for (index, line) in hunk.lines.iter().enumerate() {
            let absolute = hunk.unified_diff_start + index as u32;
            match line.kind {
                DiffLineKind::Hunk => continue,
                DiffLineKind::Context => {
                    push_line(&mut buf, b' ', line, options);
                    old_count += 1;
                    new_count += 1;
                }
                DiffLineKind::Add | DiffLineKind::Delete if selection.is_selected(absolute) => {
                    if line.kind == DiffLineKind::Add {
                        push_line(&mut buf, b'-', line, options);
                        new_count += 1;
                    } else {
                        push_line(&mut buf, b'+', line, options);
                        old_count += 1;
                    }
                    any_change = true;
                }
                // An unselected addition is already in the working copy: context.
                DiffLineKind::Add => {
                    push_line(&mut buf, b' ', line, options);
                    old_count += 1;
                    new_count += 1;
                }
                // An unselected deletion is not in the working copy: skipped.
                DiffLineKind::Delete => continue,
            }
            if line.no_trailing_newline {
                buf.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
        if !any_change {
            continue;
        }
        // The working copy is the "old" side of this reverse patch.
        let produced_start = if options.exact_hunk_starts {
            shifted_start(hunk.new_start, new_count, delta, old_count)
        } else {
            hunk.old_start
        };
        delta += i64::from(old_count) - i64::from(new_count);
        patch.extend_from_slice(
            format_hunk_header(hunk.new_start, new_count, produced_start, old_count).as_bytes(),
        );
        patch.extend_from_slice(&buf);
    }
    if patch.is_empty() {
        return None;
    }
    let mut out = format_patch_header(Some(path), Some(path)).into_bytes();
    out.extend_from_slice(&patch);
    Some(out)
}

/// GHD `discardChangesFromSelection`: `git apply --unidiff-zero --whitespace=nowarn -`.
pub fn discard_changes_from_selection(
    git: Arc<GitBinary>,
    workdir: &Path,
    patch: impl AsRef<[u8]>,
) -> Result<()> {
    GitCommand::new(git)
        .args(["apply", "--unidiff-zero", "--whitespace=nowarn", "-"])
        .current_dir(workdir)
        .stdin(patch.as_ref().to_vec())
        .run()?;
    Ok(())
}

/// `git apply --cached --unidiff-zero --whitespace=nowarn` with the partial patch.
pub fn apply_patch_to_index(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
    diff: &Diff,
    options: PatchOptions,
) -> Result<()> {
    recreate_rename_in_index(git.clone(), workdir, file)?;
    apply_hunks_to_index(git, workdir, file, diff, options)
}

/// Recreate a renamed file's rename in the (just reset) index: `git mv` by
/// hand, so HEAD's blob of the old path sits at the new path and the partial
/// patch applies against it. A no-op for other files.
fn recreate_rename_in_index(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
) -> Result<()> {
    if file.status.kind == FileStatusKind::Renamed
        && let Some(old_path) = &file.old_path
    {
        GitCommand::new(git.clone())
            .args(["add", "--update", "--", old_path])
            .current_dir(workdir)
            .run()?;
        let out = GitCommand::new(git.clone())
            .args(["ls-tree", "HEAD", "--", old_path])
            .current_dir(workdir)
            .run()?;
        let text = out.stdout_string()?;
        let info = text.split('\t').next().unwrap_or_default();
        let mut parts = info.split(' ');
        let (mode, _, oid) = (
            parts.next().unwrap_or_default(),
            parts.next(),
            parts.next().unwrap_or_default(),
        );
        GitCommand::new(git.clone())
            .args([
                "update-index",
                "--add",
                "--cacheinfo",
                mode,
                oid,
                &file.path,
            ])
            .current_dir(workdir)
            .run()?;
    }
    Ok(())
}

/// `git apply --cached` with the selected lines of `diff`.
fn apply_hunks_to_index(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
    diff: &Diff,
    options: PatchOptions,
) -> Result<()> {
    let hunks = match diff {
        Diff::Text { hunks, warnings } | Diff::LargeText { hunks, warnings } => {
            if let Some(utf16) = &warnings.utf16 {
                return crate::utf16::stage_selection(git, workdir, file, hunks, utf16);
            }
            hunks
        }
        // `1324-document-diff`: lines of an RTF file's own diff
        Diff::Document {
            source: Some(source),
            ..
        } => {
            return apply_hunks_to_index(git, workdir, file, source, options);
        }
        // `1331-notebook-diff` / `1332-latex-diff`: lines of git's own diff
        Diff::Notebook { source, .. } | Diff::Latex { source, .. } => {
            return apply_hunks_to_index(git, workdir, file, source, options);
        }
        Diff::Binary
        | Diff::Image { .. }
        | Diff::Table { .. }
        | Diff::Archive(_)
        | Diff::Font { .. }
        | Diff::FontSource { .. }
        | Diff::Document { .. }
        | Diff::Submodule(_) => {
            return Err(GitError::Gix(format!(
                "Can't create partial commit in binary file: {}",
                file.path
            )));
        }
        Diff::TooLarge => {
            return Err(GitError::Gix(format!(
                "File diff is too large to generate a partial commit: {}",
                file.path
            )));
        }
        Diff::Empty => return Ok(()),
    };
    let Some(patch) = format_patch_with(file, hunks, options) else {
        return Ok(());
    };
    GitCommand::new(git)
        .args([
            "apply",
            "--cached",
            "--unidiff-zero",
            "--whitespace=nowarn",
            "-",
        ])
        .current_dir(workdir)
        .stdin(patch)
        .run()?;
    Ok(())
}

/// Stage the partially-selected files by patch (GHD `stageFiles`, step 3),
/// with GHD's patch ([`stage_partial_files_with`]).
pub fn stage_partial_files(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
) -> Result<()> {
    stage_partial_files_with(git, workdir, files, PatchOptions::default())
}

/// [`stage_partial_files`] with Corvene's [`PatchOptions`].
pub fn stage_partial_files_with(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    options: PatchOptions,
) -> Result<()> {
    stage_partial_files_with_progress(git, workdir, files, options, &mut |_| {})
}

/// Corvene `1307-commit-progress`: [`stage_partial_files_with`], telling
/// `on_staged` how many of the partially-included files are in the index
/// after each one.
pub fn stage_partial_files_with_progress(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    options: PatchOptions,
    on_staged: &mut dyn FnMut(usize),
) -> Result<()> {
    for (index, file) in files
        .iter()
        .filter(|f| f.selection.kind() == DiffSelectionType::Partial)
        .enumerate()
    {
        // GHD `applyPatchToIndex`: the rename goes back into the index before
        // the diff is taken, so a renamed file diffs HEAD's old blob (now at
        // the new path) against the working copy (after the reset the new
        // path is untracked and `diff -- path` would be empty).
        recreate_rename_in_index(git.clone(), workdir, file)?;
        let diff = crate::diff::working_directory_diff(
            git.clone(),
            workdir,
            file,
            false,
            false,
            false,
            None,
        )?;
        apply_hunks_to_index(git.clone(), workdir, file, &diff, options)?;
        on_staged(index + 1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvene_models::{DiffSelection, FileStatus, GitStatusEntry};
    use std::process::Command;

    fn file(
        path: &str,
        kind: FileStatusKind,
        selection: DiffSelection,
    ) -> WorkingDirectoryFileChange {
        WorkingDirectoryFileChange {
            path: path.into(),
            old_path: None,
            status: FileStatus {
                kind,
                submodule: false,
                submodule_status: None,
                index: GitStatusEntry::Unchanged,
                working_tree: GitStatusEntry::Modified,
                score: None,
                code: String::new(),
                conflict_markers: None,
            },
            selection,
        }
    }

    #[test]
    fn formats_selected_lines_only() {
        let diff = crate::parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n@@ -10,2 +10,3 @@\n ten\n+eleven\n twelve\n",
        );
        let Diff::Text { hunks, .. } = &diff else {
            panic!("text diff expected")
        };
        // lines: 0 hunk,1 ctx,2 del,3 add,4 ctx | 5 hunk,6 ctx,7 add,8 ctx
        let sel = DiffSelection::none().with_line(3, true);
        let f = file("f", FileStatusKind::Modified, sel);
        let patch = format_patch(&f, hunks).unwrap();
        assert_eq!(
            patch,
            "--- a/f\n+++ b/f\n@@ -1,3 +1,4 @@\n one\n two\n+TWO\n three\n"
        );
        let none = file("f", FileStatusKind::Modified, DiffSelection::none());
        assert!(format_patch(&none, hunks).is_none());
        let sel = DiffSelection::all().with_line(7, false);
        let f = file("f", FileStatusKind::Modified, sel);
        let patch = format_patch(&f, hunks).unwrap();
        assert_eq!(
            patch,
            "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n"
        );
    }

    #[test]
    fn exact_hunk_starts_count_only_written_hunks() {
        let diff = crate::parse_unified(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,4 @@\n one\n+new1\n+new2\n two\n@@ -20,3 +22,2 @@\n p\n-q\n p\n@@ -30,2 +31,3 @@\n x\n+y\n z\n",
        );
        let Diff::Text { hunks, .. } = &diff else {
            panic!("text diff expected")
        };
        // lines: 0 hunk,1 ctx,2 add,3 add,4 ctx | 5 hunk,6 ctx,7 del,8 ctx | 9 hunk,10 ctx,11 add,12 ctx
        let sel = DiffSelection::none().with_line(7, true).with_line(11, true);
        let f = file("f", FileStatusKind::Modified, sel.clone());
        // GHD: the working copy's starts
        let ghd = format_patch(&f, hunks).unwrap();
        assert!(ghd.contains("@@ -20,3 +22,2 @@"), "{ghd}");
        let exact = PatchOptions {
            exact_hunk_starts: true,
            ..Default::default()
        };
        let patch = String::from_utf8(format_patch_with(&f, hunks, exact).unwrap()).unwrap();
        assert!(patch.contains("@@ -20,3 +20,2 @@"), "{patch}");
        assert!(patch.contains("@@ -30,2 +29,3 @@"), "{patch}");
        // discarding: the working copy is the old side, HEAD's starts move
        let patch = String::from_utf8(
            format_patch_to_discard_changes_with("f", hunks, &sel, exact).unwrap(),
        )
        .unwrap();
        assert!(patch.contains("@@ -22,2 +22,3 @@"), "{patch}");
        assert!(patch.contains("@@ -31,3 +32,2 @@"), "{patch}");
        assert_eq!(shifted_start(0, 0, 0, 3), 1);
        assert_eq!(shifted_start(5, 0, 2, 0), 7);
    }

    #[test]
    fn exact_hunk_starts_stage_the_selected_spot() {
        // #12604: unselected additions above a hunk whose lines repeat below
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        let block = "p\np\np\nq\np\np\np\n";
        let filler =
            |prefix: &str| -> String { (1..=11).map(|i| format!("{prefix}{i}\n")).collect() };
        let head = format!(
            "top\n{block}{}{block}{}{block}end\n",
            filler("f"),
            filler("g")
        );
        std::fs::write(path.join("f.txt"), &head).unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e.x",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        // 18 new lines at the top, the second block's q removed
        let added: String = (1..=18).map(|i| format!("new{i}\n")).collect();
        let second_q = head.match_indices("q\n").nth(1).unwrap().0;
        let edited = format!(
            "top\n{added}{}{}",
            &head[4..second_q],
            &head[second_q + 2..]
        );
        std::fs::write(path.join("f.txt"), &edited).unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let staged_q_lines = |options: PatchOptions| -> Vec<usize> {
            let mut status = crate::get_status(git.clone(), path).unwrap();
            let diff = crate::working_directory_diff(
                git.clone(),
                path,
                &status.files[0],
                false,
                false,
                false,
                None,
            )
            .unwrap();
            let hunks = diff.hunks().unwrap();
            let delete = hunks
                .iter()
                .flat_map(|h| {
                    h.lines
                        .iter()
                        .enumerate()
                        .map(move |(i, l)| (h.unified_diff_start + i as u32, l))
                })
                .find(|(_, l)| l.kind == DiffLineKind::Delete)
                .unwrap()
                .0;
            status.files[0].selection = DiffSelection::none().with_line(delete, true);
            crate::unstage_all(git.clone(), path).unwrap();
            stage_partial_files_with(git.clone(), path, &status.files, options).unwrap();
            let shown = Command::new("git")
                .args(["show", ":f.txt"])
                .current_dir(path)
                .output()
                .unwrap();
            String::from_utf8_lossy(&shown.stdout)
                .lines()
                .enumerate()
                .filter(|(_, l)| *l == "q")
                .map(|(i, _)| i + 1)
                .collect()
        };
        // GHD: the third block's q goes
        assert_eq!(staged_q_lines(PatchOptions::default()), [5, 23]);
        let exact = PatchOptions {
            exact_hunk_starts: true,
            ..Default::default()
        };
        assert_eq!(staged_q_lines(exact), [5, 40]);
    }

    #[test]
    fn raw_lines_stage_legacy_encoded_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        // windows-1252: "café", "naïve"
        std::fs::write(
            path.join("f.txt"),
            b"caf\xe9\none\ntwo\nthree\nfour\nfive\nna\xefve\n",
        )
        .unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e.x",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        std::fs::write(
            path.join("f.txt"),
            b"caf\xe9!\none\ntwo\nthree\nfour\nfive\nna\xefve!\n",
        )
        .unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let mut status = crate::get_status(git.clone(), path).unwrap();
        // lines: 0 hunk, 1 del café, 2 add café!, 3..7 ctx, 8 del naïve, 9 add naïve!
        status.files[0].selection = DiffSelection::none().with_range(1, 2, true);
        crate::unstage_all(git.clone(), path).unwrap();
        // GHD: the decoded text no longer matches the file
        assert!(stage_partial_files(git.clone(), path, &status.files).is_err());
        let options = PatchOptions {
            raw_lines: true,
            ..Default::default()
        };
        stage_partial_files_with(git.clone(), path, &status.files, options).unwrap();
        let shown = Command::new("git")
            .args(["show", ":f.txt"])
            .current_dir(path)
            .output()
            .unwrap();
        assert_eq!(
            shown.stdout,
            b"caf\xe9!\none\ntwo\nthree\nfour\nfive\nna\xefve\n"
        );
    }

    #[test]
    fn new_file_drops_unselected_lines() {
        let diff = crate::parse_unified("--- /dev/null\n+++ b/n\n@@ -0,0 +1,3 @@\n+a\n+b\n+c\n");
        let Diff::Text { hunks, .. } = &diff else {
            panic!("text diff expected")
        };
        let sel = DiffSelection::all().with_line(2, false);
        let f = file("n", FileStatusKind::New, sel);
        assert_eq!(
            format_patch(&f, hunks).unwrap(),
            "--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+a\n+c\n"
        );
    }

    #[test]
    fn partial_commit_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        run(&["config", "user.name", "T"]);
        run(&["config", "user.email", "t@example.com"]);
        std::fs::write(path.join("f.txt"), "one\ntwo\nthree\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(path.join("f.txt"), "ONE\ntwo\nTHREE\n").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let mut status = crate::get_status(git.clone(), path).unwrap();
        let file = &mut status.files[0];
        let diff =
            crate::working_directory_diff(git.clone(), path, file, false, false, false, None)
                .unwrap();
        // select only the first change (lines: 0 hunk, 1 del ONE, 2 add ONE, 3 ctx, 4 del, 5 add)
        file.selection = DiffSelection::none().with_range(1, 2, true);
        assert_eq!(file.selection.kind(), DiffSelectionType::Partial);
        crate::unstage_all(git.clone(), path).unwrap();
        crate::stage_files(git.clone(), path, &status.files).unwrap();
        stage_partial_files(git.clone(), path, &status.files).unwrap();
        let _ = diff;
        crate::commit(git.clone(), path, "partial\n", &Default::default()).unwrap();
        let shown = Command::new("git")
            .args(["show", "HEAD:f.txt"])
            .current_dir(path)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&shown.stdout), "ONE\ntwo\nthree\n");
        let after = crate::get_status(git, path).unwrap();
        assert_eq!(after.files.len(), 1, "the other change stays unstaged");
    }

    #[test]
    fn renamed_file_diff_and_partial_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        run(&["config", "user.name", "T"]);
        run(&["config", "user.email", "t@example.com"]);
        let body = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        std::fs::write(path.join("old.txt"), body).unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        // rename, stage an edit, then edit the working copy again
        run(&["mv", "old.txt", "new.txt"]);
        std::fs::write(path.join("new.txt"), body.replace("a\n", "A\n")).unwrap();
        run(&["add", "new.txt"]);
        std::fs::write(
            path.join("new.txt"),
            body.replace("a\n", "A\n").replace("j\n", "J\n"),
        )
        .unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let mut status = crate::get_status(git.clone(), path).unwrap();
        assert_eq!(status.files.len(), 1);
        let file = &mut status.files[0];
        assert_eq!(file.status.kind, FileStatusKind::Renamed);
        let changed = |diff: &Diff| -> Vec<String> {
            diff.hunks()
                .unwrap_or_default()
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| matches!(l.kind, DiffLineKind::Add | DiffLineKind::Delete))
                .map(|l| l.text.clone())
                .collect()
        };
        // GHD: index to working tree, the staged edit is missing
        let ghd = crate::working_directory_diff(git.clone(), path, file, false, false, false, None)
            .unwrap();
        assert_eq!(changed(&ghd), ["j", "J"]);
        // `743-renamed-diff-against-head`: HEAD's old blob to the working copy
        let diff = crate::working_directory_diff(git.clone(), path, file, false, true, false, None)
            .unwrap();
        assert_eq!(changed(&diff), ["a", "A", "j", "J"]);
        // commit only the first change (lines: 0 hunk, 1 del a, 2 add A, …)
        file.selection = DiffSelection::none().with_range(1, 2, true);
        assert_eq!(file.selection.kind(), DiffSelectionType::Partial);
        crate::unstage_all(git.clone(), path).unwrap();
        crate::stage_files(git.clone(), path, &status.files).unwrap();
        stage_partial_files(git.clone(), path, &status.files).unwrap();
        crate::commit(git.clone(), path, "partial\n", &Default::default()).unwrap();
        let show = |spec: &str| {
            Command::new("git")
                .args(["show", spec])
                .current_dir(path)
                .output()
                .unwrap()
        };
        assert!(
            !show("HEAD:old.txt").status.success(),
            "the rename is committed"
        );
        assert_eq!(
            String::from_utf8_lossy(&show("HEAD:new.txt").stdout),
            body.replace("a\n", "A\n")
        );
        let after = crate::get_status(git, path).unwrap();
        assert_eq!(after.files.len(), 1, "the other change stays unstaged");
    }
}
