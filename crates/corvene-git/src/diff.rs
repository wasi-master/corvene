//! Working-directory diffs (GHD `lib/git/diff.ts` + `lib/diff-parser.ts`):
//! text, image, submodule and large-diff detection, plus the blob / file
//! readers that back hunk expansion (`fileContents.newContents`).
//!
//! Deviations: a renamed file can diff against `HEAD:<old path>`
//! (`743-renamed-diff-against-head`); a mode-only change carries the modes
//! (`742-file-mode-change-message`); a symbolic link's working copy is its
//! target path (`745-symlink-contents`); text that is not UTF-8 can be
//! decoded in its legacy encoding (`789-non-utf8-diffs`,
//! [`crate::text_encoding`]); a UTF-16 file git takes for binary is diffed
//! as text (`1306-utf16-diffs`, [`crate::utf16`]).

use std::path::Path;
use std::sync::Arc;

use corvene_models::{
    Diff, DiffHunk, DiffLine, DiffLineKind, DiffWarnings, FileStatusKind, GitStatusEntry,
    ImageBlob, ImageOriginal, LineEndingsChange, SubmoduleDiff, SubmoduleStatus,
    WorkingDirectoryFileChange, gpui_draws_media_type, image_media_type, more_image_media_type,
};

use crate::detect::GitBinary;
use crate::error::Result;
use crate::process::{CancelToken, GitCommand};

/// GHD `MaxReasonableDiffSize`: beyond this the diff is `LargeText` and only
/// rendered on request.
pub const MAX_DIFF_BYTES: usize = 3 * 1024 * 1024;
/// GHD `MaxDiffBufferSize` (70 MB): beyond this nothing is rendered.
pub const UNRENDERABLE_BYTES: usize = 70 * 1024 * 1024;
/// Line budget after which a diff is `LargeText`.
pub const MAX_DIFF_LINES: usize = 50_000;

/// `git diff` for one working-directory file, compared against HEAD (or an
/// empty file for new/untracked files), exactly like GHD. `hide_whitespace`
/// adds `-w` (Diff Settings › Hide Whitespace Changes).
///
/// A renamed file is diffed index-to-working-tree like GHD, which hides
/// staged edits; `renamed_against_head` (Corvene `743-renamed-diff-against-head`)
/// diffs `HEAD:<old path>` to the working copy instead (`HEAD -M -- old new`),
/// the change the commit will record, falling back to GHD's diff when git
/// does not pair the two paths as one rename.
///
/// A rename in the working tree only (Corvene
/// `786-worktree-rename-detection`, index unchanged) diffs the old path's
/// blob against the working file.
///
/// `as_text` adds `--text` (Corvene `749-binary-diff-as-text`): a file git
/// takes for binary is diffed line by line anyway.
///
/// `cancel` (Corvene `764-cancel-stale-diffs`) stops the git processes when
/// another file was selected meanwhile ([`crate::GitError::Cancelled`]).
pub fn working_directory_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
    hide_whitespace: bool,
    renamed_against_head: bool,
    as_text: bool,
    cancel: Option<&CancelToken>,
) -> Result<Diff> {
    let mut args = vec!["diff"];
    if hide_whitespace {
        args.push("-w");
    }
    if as_text {
        args.push("--text");
    }
    args.extend(["--no-ext-diff", "--patch-with-raw", "-z", "--no-color"]);
    let base = || {
        let cmd = GitCommand::new(git.clone())
            .args(&args)
            .current_dir(workdir);
        match cancel {
            Some(token) => cmd.cancel_token(token.clone()),
            None => cmd,
        }
    };
    let mut cmd = base();
    let is_submodule = file.status.submodule;
    let mut rename_out = None;
    // the blob the old side comes from (`1306-utf16-diffs`): revision, path
    let mut old_side: Option<(String, String)> = None;
    if !is_submodule && file.status.kind.is_new_or_untracked() {
        // `--no-index` exits 1 when files differ, which is the normal case.
        cmd = cmd
            .args(["--no-index", "--", "/dev/null"])
            .arg(&file.path)
            .allow_exit_code(1);
    } else if file.status.kind == FileStatusKind::Renamed
        && file.status.index == GitStatusEntry::Unchanged
        && let Some(old_path) = &file.old_path
    {
        // Corvene `786-worktree-rename-detection`: a rename in the working
        // tree only (`.R`), whose new path is untracked: the old path's blob
        // (`HEAD`'s, else the index's) against the working file, hashed
        // into the object database (`hash-object -w`, so git can diff the
        // two blobs and the file's filters apply)
        let new_blob = GitCommand::new(git.clone())
            .args(["hash-object", "-w", "--"])
            .arg(&file.path)
            .current_dir(workdir)
            .run()?
            .stdout_string()?
            .trim()
            .to_string();
        let in_head = format!("HEAD:{old_path}");
        let old_rev = if GitCommand::new(git.clone())
            .args(["rev-parse", "--verify", "-q"])
            .arg(&in_head)
            .current_dir(workdir)
            .allow_exit_code(1)
            .run()?
            .status
            .success()
        {
            "HEAD"
        } else {
            ""
        };
        cmd = cmd.arg(format!("{old_rev}:{old_path}")).arg(new_blob);
        old_side = Some((old_rev.to_string(), old_path.clone()));
    } else if file.status.kind == FileStatusKind::Renamed {
        if renamed_against_head && let Some(old_path) = &file.old_path {
            let out = base()
                .args(["-M", "HEAD", "--"])
                .arg(old_path)
                .arg(&file.path)
                .run()?;
            // one patch: git paired the paths (a delete plus an add otherwise)
            if String::from_utf8_lossy(&out.stdout)
                .matches("diff --git ")
                .count()
                == 1
            {
                rename_out = Some(out);
                old_side = Some(("HEAD".to_string(), old_path.clone()));
            }
        }
        cmd = cmd.args(["--"]).arg(&file.path);
        if old_side.is_none() {
            old_side = Some((String::new(), file.path.clone()));
        }
    } else if file.status.index == GitStatusEntry::Unchanged && !is_submodule {
        // nothing staged: the index holds HEAD's blob, so index → working
        // tree prints what GHD's `HEAD -- path` prints, and git answers it
        // without `diff-index` walking the whole index (that walk took 2.2 s
        // on a 50,000-file index in git 2.54, `unpack_trees`)
        cmd = cmd.args(["--"]).arg(&file.path);
        old_side = Some((String::new(), file.path.clone()));
    } else {
        cmd = cmd.args(["HEAD", "--"]).arg(&file.path);
        old_side = Some(("HEAD".to_string(), file.path.clone()));
    }
    let out = match rename_out {
        Some(out) => out,
        None => cmd.run()?,
    };
    if is_submodule {
        return Ok(submodule_diff(
            git,
            workdir,
            &file.path,
            file.status.submodule_status.unwrap_or_default(),
            file.status.kind,
            &out.stdout,
        ));
    }
    let diff = parse_raw_diff_with_warnings(&out.stdout, &out.stderr);
    Ok(match diff {
        // `1306-utf16-diffs`: a UTF-16 text file (images stay images)
        Diff::Binary
            if binary_media_type(&file.path).is_none()
                && crate::table_file::table_format(&file.path).is_none()
                && crate::archive::archive_format(&file.path).is_none()
                && !corvene_models::is_font_file(&file.path)
                && !crate::document_file::is_binary_document(&file.path)
                && crate::utf16::decode_utf16() =>
        {
            use crate::utf16::Side;
            let working = workdir.join(&file.path);
            let new = if file.status.kind == FileStatusKind::Deleted {
                Side::Missing
            } else {
                Side::File(&working)
            };
            let old = match &old_side {
                Some((rev, path)) => Side::Blob(rev, path),
                None => Side::Missing,
            };
            let old_source = old_side.as_ref().map(|(rev, path)| format!("{rev}:{path}"));
            crate::utf16::text_diff(
                git,
                workdir,
                new,
                old,
                hide_whitespace,
                &out.stdout,
                old_source,
            )
            .unwrap_or(Diff::Binary)
        }
        diff if matches!(diff, Diff::Binary) || pdf_as_text(&file.path, &diff) => {
            let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
            // `1323-archive-diff`: both sides as git compared them
            let working = workdir.join(&file.path);
            let archive = crate::archive::archive_diff(
                git.clone(),
                workdir,
                &file.path,
                if file.status.kind == FileStatusKind::Deleted {
                    crate::archive::SideRef::Missing
                } else {
                    crate::archive::SideRef::File(&working)
                },
                match &old_side {
                    _ if file.status.kind.is_new_or_untracked() => crate::archive::SideRef::Missing,
                    Some((rev, path)) => crate::archive::SideRef::Blob(rev, path),
                    None => crate::archive::SideRef::Blob("HEAD", previous_path),
                },
            );
            if let Some(archive) = archive {
                return Ok(archive);
            }
            image_diff(
                &file.path,
                file.status.kind,
                || std::fs::read(workdir.join(&file.path)).ok(),
                || blob_bytes(git.clone(), workdir, "HEAD", previous_path).ok(),
            )
        }
        // `1324-document-diff`: an RTF or flat OpenDocument file, read into
        // blocks next to git's own diff
        diff @ (Diff::Text { .. } | Diff::LargeText { .. })
            if crate::document_file::is_text_document(&file.path) =>
        {
            let previous_path = file.old_path.as_deref().unwrap_or(&file.path);
            crate::document_file::document_diff(
                &file.path,
                file.status.kind,
                || std::fs::read(workdir.join(&file.path)).ok(),
                || blob_bytes(git.clone(), workdir, "HEAD", previous_path).ok(),
                Some(diff.clone()),
            )
            .unwrap_or(diff)
        }
        other => other,
    })
}

/// `git diff --no-index` of two versions held in memory (`None` for a
/// side that does not exist), run in a temporary directory so none of the
/// repository's attributes or filters apply. `None` when git did not run.
/// Used for decoded UTF-16 files (`1306-utf16-diffs`) and archive entries
/// (`1323-archive-diff`).
pub(crate) fn no_index_diff(
    git: Arc<GitBinary>,
    old: Option<&[u8]>,
    new: Option<&[u8]>,
    hide_whitespace: bool,
) -> Option<Diff> {
    let dir = tempfile::tempdir().ok()?;
    let write = |name: &str, bytes: Option<&[u8]>| -> Option<std::path::PathBuf> {
        let Some(bytes) = bytes else {
            return Some("/dev/null".into());
        };
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).ok()?;
        Some(path)
    };
    let old_path = write("old", old)?;
    let new_path = write("new", new)?;
    let mut args = vec!["diff", "--no-index", "--no-ext-diff"];
    if hide_whitespace {
        args.push("-w");
    }
    args.extend(["--patch-with-raw", "-z", "--no-color", "--"]);
    let out = GitCommand::new(git)
        .args(args)
        .arg(old_path)
        .arg(new_path)
        .current_dir(dir.path())
        .allow_exit_code(1)
        .run()
        .ok()?;
    Some(parse_raw_diff(&out.stdout))
}

/// Corvene `762-too-large-diff-escape-hatch`: `git difftool -y` on one
/// working-directory file against HEAD (a new file against `/dev/null`),
/// with the repository's `diff.tool`. Blocks until the tool exits. Refuses
/// without a configured `diff.tool` (git would fall back to a terminal tool).
pub fn open_difftool(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
) -> Result<()> {
    if crate::config_value(git.clone(), workdir, "diff.tool").is_none() {
        return Err(crate::error::GitError::Gix(
            "No diff tool is configured. Set one with git config --global diff.tool <tool>.".into(),
        ));
    }
    let cmd = GitCommand::new(git)
        .args(["difftool", "-y"])
        .current_dir(workdir);
    let cmd = if file.status.kind.is_new_or_untracked() {
        // `--no-index` exits 1 when the files differ
        cmd.args(["--no-index", "--", "/dev/null"])
            .arg(&file.path)
            .allow_exit_code(1)
    } else {
        cmd.args(["HEAD", "--"]).arg(&file.path)
    };
    cmd.run().map(|_| ())
}

/// Corvene `1208-undo-restores-line-selection`: the lines one file's
/// change in a commit made, in the numbering of a working-directory diff
/// against the commit's parent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitLines {
    /// The parent's lines the commit deleted.
    pub deleted: std::collections::BTreeSet<u32>,
    /// The working copy's lines that are lines the commit added (unchanged
    /// since).
    pub added: std::collections::BTreeSet<u32>,
}

/// Commits with more files than this are not read line by line.
pub const COMMIT_LINES_MAX_FILES: usize = 500;

/// Corvene `1208-undo-restores-line-selection`: for each file `commit`
/// changed against `parent`, the lines it made ([`CommitLines`]). The
/// commit's own diff comes from git; its lines are found in the working
/// copy by a line diff of the committed file against it. Read-only. `None`
/// for a commit of more than [`COMMIT_LINES_MAX_FILES`] files.
pub fn lines_made_by_commit(
    git: Arc<GitBinary>,
    workdir: &Path,
    parent: &str,
    commit: &str,
) -> Result<Option<std::collections::HashMap<String, CommitLines>>> {
    let names = GitCommand::new(git.clone())
        .args([
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--name-only",
            "-z",
            parent,
            commit,
        ])
        .current_dir(workdir)
        .run()?;
    let names = String::from_utf8_lossy(&names.stdout).into_owned();
    let paths: Vec<&str> = names.split('\0').filter(|p| !p.is_empty()).collect();
    if paths.len() > COMMIT_LINES_MAX_FILES {
        return Ok(None);
    }
    let mut out = std::collections::HashMap::new();
    for path in paths {
        let patch = GitCommand::new(git.clone())
            .args([
                "diff",
                "--no-ext-diff",
                "--no-color",
                "--no-renames",
                "-U0",
                parent,
                commit,
                "--",
                path,
            ])
            .current_dir(workdir)
            .run()?;
        let diff = parse_raw_diff(&patch.stdout);
        let Some(hunks) = diff.hunks() else {
            continue;
        };
        let lines = || hunks.iter().flat_map(|h| h.lines.iter());
        let deleted = lines()
            .filter(|l| l.kind == DiffLineKind::Delete)
            .filter_map(|l| l.old_line)
            .collect();
        let added_in_commit: std::collections::BTreeSet<u32> = lines()
            .filter(|l| l.kind == DiffLineKind::Add)
            .filter_map(|l| l.new_line)
            .collect();
        let committed = blob_bytes(git.clone(), workdir, commit, path)
            .map(|b| file_lines(&b))
            .unwrap_or_default();
        let working = working_file_lines(workdir, path, true).unwrap_or_default();
        let added = unchanged_line_pairs(&committed, &working)
            .filter(|(committed, _)| added_in_commit.contains(committed))
            .map(|(_, working)| working)
            .collect();
        out.insert(path.to_string(), CommitLines { deleted, added });
    }
    Ok(Some(out))
}

/// The lines `old` and `new` have in common (a line diff), as 1-based
/// (old line, new line) pairs.
fn unchanged_line_pairs<'a>(
    old: &'a [String],
    new: &'a [String],
) -> impl Iterator<Item = (u32, u32)> + 'a {
    let deadline = std::time::Instant::now().checked_add(std::time::Duration::from_millis(200));
    similar::capture_diff_slices_deadline(similar::Algorithm::Myers, old, new, deadline)
        .into_iter()
        .filter_map(|op| match op {
            similar::DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => Some(
                (0..len).map(move |k| ((old_index + k + 1) as u32, (new_index + k + 1) as u32)),
            ),
            _ => None,
        })
        .flatten()
}

/// Corvene `793-hide-whitespace-only-files`: the files `git diff <revs>`
/// (`HEAD` for the working directory, `<base> <commit>` for a commit)
/// changes in whitespace only: modified in place (neither new, deleted,
/// renamed, a submodule nor changed in type or mode) and without a changed
/// line once whitespace is ignored. `git diff -w` leaves such files out
/// (older versions list them with 0 lines), so the file list without `-w`
/// is compared with the one with it. Read-only.
pub fn whitespace_only_paths(
    git: Arc<GitBinary>,
    workdir: &Path,
    revs: &[&str],
) -> Result<std::collections::HashSet<String>> {
    let run = |args: &[&str]| {
        GitCommand::new(git.clone())
            .args(["diff", "--no-ext-diff", "--no-color", "--no-renames", "-z"])
            .args(args)
            .args(revs)
            .arg("--")
            .current_dir(workdir)
            .run()
    };
    let raw = run(&["--raw"])?;
    let ignoring_whitespace = run(&["-w", "--numstat"])?;
    Ok(parse_whitespace_only(
        &raw.stdout,
        &ignoring_whitespace.stdout,
    ))
}

/// [`whitespace_only_paths`]' parser: `raw` is `--raw -z` output, `numstat`
/// is `-w --numstat -z` output.
fn parse_whitespace_only(raw: &[u8], numstat: &[u8]) -> std::collections::HashSet<String> {
    let raw = String::from_utf8_lossy(raw);
    let mut fields = raw.split('\0');
    let mut modified = std::collections::HashSet::new();
    while let Some(field) = fields.next() {
        let Some(record) = field.strip_prefix(':') else {
            continue;
        };
        // ":100644 100644 <old> <new> M", then the path
        let Some(path) = fields.next() else { break };
        let parts: Vec<&str> = record.split(' ').collect();
        if let [old_mode, new_mode, _, _, status, ..] = parts[..]
            && old_mode == new_mode
            && old_mode != "160000"
            && status == "M"
        {
            modified.insert(path.to_string());
        }
    }
    let numstat = String::from_utf8_lossy(numstat);
    for record in numstat.split('\0') {
        let mut parts = record.splitn(3, '\t');
        if let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
            && (added, deleted) != ("0", "0")
        {
            modified.remove(path);
        }
    }
    modified
}

/// Corvene `714-copy-diff`: the working-directory changes of `files` as one
/// patch `git apply` takes (`--binary`), against `base` (`HEAD`, or
/// [`crate::NULL_TREE_SHA`] on an unborn branch). Tracked files come first,
/// in one `git diff`, then each new / untracked file against `/dev/null`.
pub fn working_directory_patch(
    git: Arc<GitBinary>,
    workdir: &Path,
    files: &[WorkingDirectoryFileChange],
    base: &str,
) -> Result<String> {
    const ARGS: [&str; 4] = ["diff", "--no-ext-diff", "--no-color", "--binary"];
    let (untracked, tracked): (Vec<_>, Vec<_>) = files
        .iter()
        .partition(|f| !f.status.submodule && f.status.kind.is_new_or_untracked());
    let mut patch = Vec::new();
    // in batches: every path of a huge change set would pass the system's
    // argument size limit
    for batch in tracked.chunks(1000) {
        let paths = batch
            .iter()
            .flat_map(|f| std::iter::once(&f.path).chain(f.old_path.as_ref()));
        let out = GitCommand::new(git.clone())
            .args(ARGS)
            .args([base, "--"])
            .args(paths)
            .current_dir(workdir)
            .run()?;
        patch.extend(out.stdout);
    }
    for f in untracked {
        // `--no-index` exits 1 when the files differ
        let out = GitCommand::new(git.clone())
            .args(ARGS)
            .args(["--no-index", "--", "/dev/null"])
            .arg(&f.path)
            .current_dir(workdir)
            .allow_exit_code(1)
            .run()?;
        patch.extend(out.stdout);
    }
    Ok(String::from_utf8_lossy(&patch).into_owned())
}

/// GHD `getImageDiff`: a binary change of a known image type becomes an
/// image diff; anything else stays `Binary` (a spreadsheet or database
/// becomes a table diff, Corvene `1321-table-file-diff`; a font a font
/// diff, Corvene `1329-font-diff`; a Word, PowerPoint, OpenDocument or
/// EPUB file a document diff, Corvene `1324-document-diff`).
pub fn image_diff(
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
) -> Diff {
    let Some(media_type) = binary_media_type(path) else {
        if corvene_models::is_font_file(path) {
            return crate::font_file::font_diff(path, kind, current, previous)
                .unwrap_or(Diff::Binary);
        }
        if crate::document_file::document_format(path).is_some() {
            return crate::document_file::document_diff(path, kind, current, previous, None)
                .unwrap_or(Diff::Binary);
        }
        return crate::table_file::table_diff(path, kind, current, previous)
            .unwrap_or(Diff::Binary);
    };
    if media_type == PDF_MEDIA_TYPE {
        let small = |bytes: Vec<u8>| (bytes.len() <= PDF_MAX_BYTES).then_some(bytes);
        let diff = image_diff_as(
            media_type,
            kind,
            || current().and_then(small),
            || previous().and_then(small),
        );
        // one side too large: the whole file stays binary
        let both = !kind.is_new_or_untracked() && kind != FileStatusKind::Deleted;
        return match diff {
            Diff::Image {
                previous: None,
                current: Some(_),
            }
            | Diff::Image {
                previous: Some(_),
                current: None,
            } if both => Diff::Binary,
            diff => diff,
        };
    }
    image_diff_as(media_type, kind, current, previous)
}

/// [`image_media_type`], plus [`PDF_MEDIA_TYPE`] for a PDF file while
/// `1322-pdf-diff` is on and [`more_image_media_type`] while an image
/// decoder is set (`1326-more-image-formats`): the binary files shown by
/// the image diff.
pub fn binary_media_type(path: &str) -> Option<&'static str> {
    image_media_type(path)
        .or_else(|| image_decoder().and(more_image_media_type(path)))
        .or_else(|| (pdf_diffs() && is_pdf(path)).then_some(PDF_MEDIA_TYPE))
}

/// Corvene `1326-more-image-formats`: an image file decoded for display.
pub struct DecodedImage {
    /// At most a few thousand pixels on its longer side.
    pub png: Vec<u8>,
    /// The image's own size in pixels.
    pub width: u32,
    pub height: u32,
}

/// Corvene `1326-more-image-formats`: decodes a file of the given media
/// type (`corvene_platform::image_decode`, set by the dispatcher).
pub type ImageDecoder = fn(&[u8], &str) -> Option<DecodedImage>;

static IMAGE_DECODER: std::sync::RwLock<Option<ImageDecoder>> = std::sync::RwLock::new(None);

/// Show the images GPUI cannot draw by decoding them with `decoder`, or
/// as binary files with `None` (flag `1326-more-image-formats`).
pub fn set_image_decoder(decoder: Option<ImageDecoder>) {
    if let Ok(mut slot) = IMAGE_DECODER.write() {
        *slot = decoder;
    }
}

fn image_decoder() -> Option<ImageDecoder> {
    IMAGE_DECODER.read().ok().and_then(|slot| *slot)
}

/// Corvene `1326-more-image-formats`: replaces `blob`'s bytes with a PNG
/// GPUI draws when its media type is one GPUI cannot draw; false when it
/// needed decoding and could not be.
pub(crate) fn decode_image_blob(blob: &mut ImageBlob) -> bool {
    if blob.original.is_some() || gpui_draws_media_type(&blob.media_type) {
        return true;
    }
    let Some(decoder) = image_decoder() else {
        return true;
    };
    let Some(decoded) = decoder(&blob.bytes, &blob.media_type) else {
        return false;
    };
    let bytes = std::mem::replace(&mut blob.bytes, decoded.png);
    blob.original = Some(ImageOriginal {
        media_type: std::mem::replace(&mut blob.media_type, "image/png".to_string()),
        bytes: bytes.len(),
        width: decoded.width,
        height: decoded.height,
    });
    true
}

/// Corvene `1322-pdf-diff`: a PDF file's media type in an image diff. GHD
/// (`lib/git/diff.ts` `getDiffType`, `imageFileExtensions`) shows a PDF as
/// "This binary file has changed." (`ui/diff/binary-file.tsx`).
pub const PDF_MEDIA_TYPE: &str = "application/pdf";

/// Corvene `1322-pdf-diff`: larger PDF files stay binary.
pub const PDF_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Set by the dispatcher from the flag.
static PDF_DIFFS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turn PDF page diffs on or off (flag `1322-pdf-diff`).
pub fn set_pdf_diffs(enabled: bool) {
    PDF_DIFFS.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn pdf_diffs() -> bool {
    PDF_DIFFS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Corvene `1322-pdf-diff`: whether `path` is a PDF file.
pub fn is_pdf(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

/// Corvene `1322-pdf-diff`: a PDF file that git diffs as text (one saved
/// without binary bytes) is shown as pages too.
pub(crate) fn pdf_as_text(path: &str, diff: &Diff) -> bool {
    pdf_diffs() && is_pdf(path) && matches!(diff, Diff::Text { .. } | Diff::LargeText { .. })
}

/// Corvene `794-svg-image-diff`: an SVG file's media type when it is shown
/// as an image (GHD only ever shows its text).
pub const SVG_MEDIA_TYPE: &str = "image/svg+xml";

/// Corvene `794-svg-image-diff`: whether `path` is an SVG file.
pub fn is_svg(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
}

/// [`image_diff`] of a file known to be an image of `media_type`: the sides
/// that exist (`Binary` when neither does).
pub fn image_diff_as(
    media_type: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
) -> Diff {
    let blob = |bytes: Vec<u8>| ImageBlob {
        bytes,
        media_type: media_type.to_string(),
        original: None,
    };
    let mut current = (kind != FileStatusKind::Deleted)
        .then(current)
        .flatten()
        .map(blob);
    let mut previous = (!kind.is_new_or_untracked())
        .then(previous)
        .flatten()
        .map(blob);
    if current.is_none() && previous.is_none() {
        return Diff::Binary;
    }
    // `1326-more-image-formats`: both sides decoded at once; an LFS pointer
    // is decoded once it is swapped for its object (`lfs::resolve_lfs_images`)
    if !gpui_draws_media_type(media_type) && image_decoder().is_some() {
        let decode = |side: &mut Option<ImageBlob>| match side {
            Some(blob) if crate::lfs::lfs_pointer(&blob.bytes).is_none() => {
                Some(decode_image_blob(blob))
            }
            _ => None,
        };
        let (a, b) = std::thread::scope(|scope| {
            let a = scope.spawn(|| decode(&mut previous));
            let b = decode(&mut current);
            (a.join().ok().flatten(), b)
        });
        // nothing this platform decodes: GHD's binary file
        if a != Some(true) && b != Some(true) && (a.is_some() || b.is_some()) {
            return Diff::Binary;
        }
    }
    Diff::Image { previous, current }
}

/// GHD `buildSubmoduleDiff`: the `Subproject commit` lines of the patch plus
/// `submodule.<path>.url` from the repository config.
pub fn submodule_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    status: SubmoduleStatus,
    kind: FileStatusKind,
    patch: &[u8],
) -> Diff {
    let url = crate::config::local_config_value(git, workdir, &format!("submodule.{path}.url"));
    let (mut old_sha, mut new_sha) = (None, None);
    if status.commit_changed || kind == FileStatusKind::New || kind == FileStatusKind::Deleted {
        let text = String::from_utf8_lossy(patch);
        let sha = |rest: &str| rest.trim_end().trim_end_matches("-dirty").to_string();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("-Subproject commit ") {
                old_sha.get_or_insert_with(|| sha(rest));
            } else if let Some(rest) = line.strip_prefix("+Subproject commit ") {
                new_sha.get_or_insert_with(|| sha(rest));
            }
        }
    }
    Diff::Submodule(SubmoduleDiff {
        path: path.to_string(),
        full_path: workdir.join(path),
        url,
        old_sha,
        new_sha,
        status,
    })
}

/// `git show <commitish>:<path>` - the raw bytes of a blob (`getBlobContents`).
pub fn blob_bytes(
    git: Arc<GitBinary>,
    workdir: &Path,
    commitish: &str,
    path: &str,
) -> Result<Vec<u8>> {
    // in-process first: a `git show` spawn costs 5–10 ms, more under load
    if let Some(bytes) = crate::handle::blob_bytes(workdir, commitish, path) {
        return Ok(bytes);
    }
    let out = GitCommand::new(git)
        .args(["show", &format!("{commitish}:{path}")])
        .current_dir(workdir)
        .run()?;
    Ok(out.stdout)
}

/// File contents as lines for hunk expansion; a trailing newline does not
/// produce an empty last line.
/// Text that is not UTF-8 is decoded per [`crate::text_encoding`].
pub fn file_lines(bytes: &[u8]) -> Vec<String> {
    let text = crate::text_encoding::file_text(bytes);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// The working copy of `path` as lines (`None` when unreadable, e.g. deleted).
///
/// GHD reads through a symbolic link, which hangs on a link to a FIFO or a
/// device and loads a huge target whole; `symlinks_as_links` (Corvene
/// `745-symlink-contents`) reads the link's target path instead, the one
/// line git records and diffs for a link.
pub fn working_file_lines(
    workdir: &Path,
    path: &str,
    symlinks_as_links: bool,
) -> Option<Vec<String>> {
    let full = workdir.join(path);
    if symlinks_as_links
        && std::fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink())
    {
        let target = std::fs::read_link(&full).ok()?;
        return Some(file_lines(target.to_string_lossy().as_bytes()));
    }
    std::fs::read(full).ok().map(|b| file_lines(&b))
}

/// GHD `readPartialFile` (`lib/file-system.ts`): bytes `start..=end` of the
/// file at `path` (fewer when the file ends first).
pub fn read_partial_file(path: &Path, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(end.saturating_add(1).saturating_sub(start))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A committed blob as lines (`None` when the path is not in that commit).
pub fn blob_lines(
    git: Arc<GitBinary>,
    workdir: &Path,
    commitish: &str,
    path: &str,
) -> Option<Vec<String>> {
    blob_bytes(git, workdir, commitish, path)
        .ok()
        .map(|b| file_lines(&b))
}

/// GHD `parseLineEndingsWarning`: git's stderr notice that the working copy's
/// line endings will be converted on checkout (both the classic and the
/// git ≥ 2.37 wording).
pub fn parse_line_endings_warning(stderr: &str) -> Option<LineEndingsChange> {
    for line in stderr.lines() {
        let line = line.trim();
        let rest = line.strip_prefix("warning: ")?;
        // "in the working copy of 'x', CRLF will be replaced by LF the next time…"
        let rest = match rest.find(", ") {
            Some(ix) if rest.starts_with("in the working copy of") => &rest[ix + 2..],
            _ => rest,
        };
        let (from, tail) = rest.split_once(" will be replaced by ")?;
        let to = tail.split([' ', '.']).next().unwrap_or("");
        let valid = |s: &str| matches!(s, "CRLF" | "LF" | "CR");
        if valid(from) && valid(to) {
            return Some(LineEndingsChange {
                from: from.to_string(),
                to: to.to_string(),
            });
        }
    }
    None
}

/// GHD `HiddenBidiCharsRegex`.
pub fn has_hidden_bidi_chars(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
}

/// [`parse_raw_diff`] plus the warnings derived from git's stderr.
pub fn parse_raw_diff_with_warnings(stdout: &[u8], stderr: &str) -> Diff {
    let line_endings = parse_line_endings_warning(stderr);
    match parse_raw_diff(stdout) {
        Diff::Text {
            hunks,
            mut warnings,
        } => {
            warnings.line_endings = line_endings;
            Diff::Text { hunks, warnings }
        }
        Diff::LargeText {
            hunks,
            mut warnings,
        } => {
            warnings.line_endings = line_endings;
            Diff::LargeText { hunks, warnings }
        }
        other => other,
    }
}

/// Parse `--patch-with-raw -z` output: skip the raw header block, then the
/// unified diff. Handles binary and oversize diffs.
pub fn parse_raw_diff(stdout: &[u8]) -> Diff {
    parse_raw_diff_decoding(stdout, crate::text_encoding::decode_legacy())
}

/// [`parse_raw_diff`]; `decode_legacy`: flag `789-non-utf8-diffs`.
fn parse_raw_diff_decoding(stdout: &[u8], decode_legacy: bool) -> Diff {
    if stdout.len() > UNRENDERABLE_BYTES {
        return Diff::TooLarge;
    }
    let text = String::from_utf8_lossy(stdout);
    // With -z the raw section is NUL separated; the patch starts at "diff --git".
    let patch_start = text.find("diff --git").unwrap_or(text.len());
    let patch = &text[patch_start..];
    if patch.trim().is_empty() {
        return Diff::Empty;
    }
    if patch.contains("\nBinary files ") || patch.starts_with("Binary files ") {
        return Diff::Binary;
    }
    // not UTF-8: the same patch's bytes, line for line (the replacement
    // characters never stand in for a newline)
    let raw = match &text {
        std::borrow::Cow::Borrowed(_) => None,
        std::borrow::Cow::Owned(_) => {
            let start = stdout
                .windows(b"diff --git".len())
                .position(|w| w == b"diff --git")
                .unwrap_or(stdout.len());
            Some(&stdout[start..])
        }
    };
    match parse_unified_with_raw(patch, raw, decode_legacy) {
        Diff::Text { hunks, warnings } if stdout.len() > MAX_DIFF_BYTES => {
            Diff::LargeText { hunks, warnings }
        }
        other => other,
    }
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    // @@ -a[,b] +c[,d] @@ ...
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let parse_range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((s, n)) => Some((s.parse().ok()?, n.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (os, ol) = parse_range(old)?;
    let (ns, nl) = parse_range(new)?;
    Some((os, ol, ns, nl))
}

pub fn parse_unified(patch: &str) -> Diff {
    parse_unified_with_raw(patch, None, false)
}

/// [`parse_unified`]; `raw` is the patch's bytes when they are not UTF-8
/// (`patch` is then their lossy rendition): changed and context lines that
/// are not UTF-8 keep their bytes (`DiffLine::raw`), and with
/// `decode_legacy` (flag `789-non-utf8-diffs`) their text is decoded in the
/// encoding guessed from them all.
fn parse_unified_with_raw(patch: &str, raw: Option<&[u8]>, decode_legacy: bool) -> Diff {
    let mut raw_lines = raw.map(|raw| raw.split_inclusive(|b| *b == b'\n'));
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut current: Option<DiffHunk> = None;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    let mut total_lines = 0usize;
    let mut truncated = false;
    let (mut old_mode, mut new_mode) = (None, None);
    // a type change prints two file sections: the deletion, then the addition
    let (mut sections, mut deleted_mode, mut added_mode) = (0usize, None, None);

    for line in patch.split_inclusive('\n') {
        let raw_line = raw_lines.as_mut().and_then(|lines| lines.next());
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("diff --git ") {
            // the next section's headers (`--- /dev/null`) are not content
            sections += 1;
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            continue;
        }
        if let Some((os, ol, ns, nl)) = parse_hunk_header(line) {
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            old_no = os;
            new_no = ns;
            current = Some(DiffHunk {
                unified_diff_start: hunks.iter().map(|h| h.lines.len() as u32).sum(),
                header: line.to_string(),
                old_start: os,
                old_lines: ol,
                new_start: ns,
                new_lines: nl,
                lines: vec![DiffLine {
                    kind: DiffLineKind::Hunk,
                    text: line.to_string(),
                    old_line: None,
                    new_line: None,
                    no_trailing_newline: false,
                    raw: None,
                }],
            });
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            // file header lines before the first hunk
            if let Some(mode) = line.strip_prefix("old mode ") {
                old_mode = Some(mode.to_string());
            } else if let Some(mode) = line.strip_prefix("new mode ") {
                new_mode = Some(mode.to_string());
            } else if let Some(mode) = line.strip_prefix("deleted file mode ") {
                deleted_mode = Some(mode.to_string());
            } else if let Some(mode) = line.strip_prefix("new file mode ") {
                added_mode = Some(mode.to_string());
            }
            continue;
        };
        if line.starts_with("\\ No newline at end of file") {
            if let Some(last) = hunk.lines.last_mut() {
                last.no_trailing_newline = true;
            }
            continue;
        }
        total_lines += 1;
        if total_lines > MAX_DIFF_LINES {
            truncated = true;
            break;
        }
        let (kind, text) = match line.chars().next() {
            Some('+') => (DiffLineKind::Add, &line[1..]),
            Some('-') => (DiffLineKind::Delete, &line[1..]),
            Some(' ') => (DiffLineKind::Context, &line[1..]),
            None => (DiffLineKind::Context, ""),
            _ => continue,
        };
        let (old_line, new_line) = match kind {
            DiffLineKind::Add => {
                let n = new_no;
                new_no += 1;
                (None, Some(n))
            }
            DiffLineKind::Delete => {
                let o = old_no;
                old_no += 1;
                (Some(o), None)
            }
            _ => {
                let (o, n) = (old_no, new_no);
                old_no += 1;
                new_no += 1;
                (Some(o), Some(n))
            }
        };
        let raw = raw_line.and_then(|bytes| {
            let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
            let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
            let bytes = bytes.get(1..)?;
            std::str::from_utf8(bytes).is_err().then(|| bytes.to_vec())
        });
        hunk.lines.push(DiffLine {
            kind,
            text: text.to_string(),
            old_line,
            new_line,
            no_trailing_newline: false,
            raw,
        });
    }
    if let Some(h) = current.take() {
        hunks.push(h);
    }
    let mode_change = old_mode.zip(new_mode);
    let type_change = if sections >= 2 {
        deleted_mode.zip(added_mode)
    } else {
        None
    };
    if hunks.is_empty() && mode_change.is_none() {
        return Diff::Empty;
    }
    if decode_legacy {
        decode_raw_lines(&mut hunks);
    }
    let warnings = DiffWarnings {
        hidden_bidi: hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .any(|l| has_hidden_bidi_chars(&l.text)),
        line_endings: None,
        mode_change,
        type_change,
        lfs_not_downloaded: false,
        lfs_contents: false,
        utf16: None,
        pdf_text: false,
        document_text: false,
    };
    if truncated {
        Diff::LargeText { hunks, warnings }
    } else {
        Diff::Text { hunks, warnings }
    }
}

/// Flag `789-non-utf8-diffs`: the text of the lines that kept their bytes,
/// decoded in the encoding guessed from all of them.
fn decode_raw_lines(hunks: &mut [DiffHunk]) {
    let samples: Vec<&[u8]> = hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter_map(|l| l.raw.as_deref())
        .collect();
    if samples.is_empty() {
        return;
    }
    let encoding = crate::text_encoding::guess_encoding(samples);
    for line in hunks.iter_mut().flat_map(|h| h.lines.iter_mut()) {
        if let Some(raw) = &line.raw {
            line.text = crate::text_encoding::decode_with(raw, encoding);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_only_files_are_plain_modifications_without_lines() {
        let raw = b":100644 100644 aaa bbb M\0ws.txt\0:100644 100755 ccc ddd M\0mode.sh\0:000000 100644 000 eee A\0new.txt\0:100644 100644 fff 111 M\0real.txt\0:100644 100644 222 333 M\0bin.png\0:100644 100644 444 555 M\0old-git.txt\0";
        // `-w`: git 2.54 leaves ws.txt out, older versions list 0 lines
        let numstat = b"0\t0\told-git.txt\x002\t1\treal.txt\0-\t-\tbin.png\0";
        let paths = parse_whitespace_only(raw, numstat);
        assert_eq!(
            paths,
            std::collections::HashSet::from(["ws.txt".to_string(), "old-git.txt".to_string()])
        );
    }

    #[test]
    fn whitespace_only_paths_in_a_repository() {
        use std::process::Command;
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
        std::fs::write(path.join("ws.txt"), "a\nb\n").unwrap();
        std::fs::write(path.join("real.txt"), "a\nb\n").unwrap();
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
        std::fs::write(path.join("ws.txt"), "  a\nb  \n").unwrap();
        std::fs::write(path.join("real.txt"), "a\nc\n").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let paths = whitespace_only_paths(git, path, &["HEAD"]).unwrap();
        assert_eq!(
            paths,
            std::collections::HashSet::from(["ws.txt".to_string()])
        );
    }

    #[test]
    fn non_utf8_lines_keep_their_bytes_and_decode() {
        let patch =
            b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n-caf\xe9\n+caf\xe9s\n ok\n";
        let lines = |diff: &Diff| -> Vec<(String, Option<Vec<u8>>)> {
            diff.hunks().unwrap()[0].lines[1..]
                .iter()
                .map(|l| (l.text.clone(), l.raw.clone()))
                .collect()
        };
        // GHD: replacement characters
        let ghd = parse_raw_diff_decoding(patch, false);
        assert_eq!(
            lines(&ghd),
            [
                ("caf\u{fffd}".to_string(), Some(b"caf\xe9".to_vec())),
                ("caf\u{fffd}s".to_string(), Some(b"caf\xe9s".to_vec())),
                ("ok".to_string(), None),
            ]
        );
        let decoded = parse_raw_diff_decoding(patch, true);
        assert_eq!(
            lines(&decoded),
            [
                ("café".to_string(), Some(b"caf\xe9".to_vec())),
                ("cafés".to_string(), Some(b"caf\xe9s".to_vec())),
                ("ok".to_string(), None),
            ]
        );
        // UTF-8 diffs keep no bytes
        let utf8 =
            parse_raw_diff_decoding("diff --git a/f b/f\n@@ -1 +1 @@\n-é\n+è\n".as_bytes(), true);
        assert!(
            utf8.hunks().unwrap()[0]
                .lines
                .iter()
                .all(|l| l.raw.is_none())
        );
    }

    #[test]
    fn working_directory_patch_applies() {
        use std::process::Command;
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
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        std::fs::write(path.join("skip.txt"), "same\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(path.join("skip.txt"), "changed\n").unwrap();
        std::fs::write(path.join("new.txt"), "fresh\n").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let status = crate::get_status(git.clone(), path).unwrap();
        let files: Vec<_> = status
            .files
            .into_iter()
            .filter(|f| f.path != "skip.txt")
            .collect();
        let patch = working_directory_patch(git, path, &files, "HEAD").unwrap();
        assert!(patch.contains("+two"));
        assert!(patch.contains("+fresh"));
        assert!(!patch.contains("skip.txt"));
        // the patch reverses cleanly onto the working tree
        std::fs::write(path.join("p.diff"), &patch).unwrap();
        run(&["apply", "--check", "-R", "p.diff"]);
    }

    #[test]
    fn difftool_runs_the_configured_tool() {
        use std::process::Command;
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
        std::fs::write(path.join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(path.join("a.txt"), "two\n").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        assert_eq!(crate::config_value(git.clone(), path, "diff.tool"), None);
        run(&["config", "diff.tool", "fake"]);
        run(&["config", "difftool.fake.cmd", "cat \"$REMOTE\" > seen.txt"]);
        assert_eq!(
            crate::config_value(git.clone(), path, "diff.tool").as_deref(),
            Some("fake")
        );
        let status = crate::get_status(git.clone(), path).unwrap();
        open_difftool(git, path, &status.files[0]).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("seen.txt")).unwrap(),
            "two\n"
        );
    }

    const SAMPLE: &str = "diff --git a/a.txt b/a.txt\nindex 1..2 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,4 @@\n one\n-two\n+TWO\n+three\n four\n\\ No newline at end of file\n";

    #[test]
    fn parses_hunks_and_line_numbers() {
        let diff = parse_unified(SAMPLE);
        let Diff::Text { hunks, .. } = diff else {
            panic!("expected text diff")
        };
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(
            (h.old_start, h.old_lines, h.new_start, h.new_lines),
            (1, 3, 1, 4)
        );
        assert_eq!(h.lines[0].kind, DiffLineKind::Hunk);
        let kinds: Vec<_> = h.lines[1..].iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::Context,
                DiffLineKind::Delete,
                DiffLineKind::Add,
                DiffLineKind::Add,
                DiffLineKind::Context
            ]
        );
        assert_eq!(h.lines[2].old_line, Some(2));
        assert_eq!(h.lines[3].new_line, Some(2));
        assert_eq!(h.lines[4].new_line, Some(3));
        assert_eq!(h.lines[5].old_line, Some(3));
        assert_eq!(h.lines[5].new_line, Some(4));
        assert!(h.lines[5].no_trailing_newline);
    }

    #[test]
    fn raw_prefix_binary_and_empty() {
        let raw = ":100644 100644 aaa bbb M\0a.txt\0\ndiff --git a/a.txt b/a.txt\nBinary files a/a.txt and b/a.txt differ\n";
        assert_eq!(parse_raw_diff(raw.as_bytes()), Diff::Binary);
        assert_eq!(parse_raw_diff(b""), Diff::Empty);
        let raw = format!(":100644 100644 aaa bbb M\0a.txt\0\n{SAMPLE}");
        assert!(matches!(parse_raw_diff(raw.as_bytes()), Diff::Text { .. }));
    }

    #[test]
    fn live_diff_for_untracked_and_modified() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(path)
                    .env("GIT_AUTHOR_NAME", "T")
                    .env("GIT_AUTHOR_EMAIL", "t@example.com")
                    .env("GIT_COMMITTER_NAME", "T")
                    .env("GIT_COMMITTER_EMAIL", "t@example.com")
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(path.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(path.join("b.txt"), "new\n").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let status = crate::status::get_status(git.clone(), path).unwrap();
        for file in &status.files {
            let diff =
                working_directory_diff(git.clone(), path, file, false, false, false, None).unwrap();
            let Diff::Text { hunks, .. } = diff else {
                panic!("text diff for {}", file.path)
            };
            assert_eq!(hunks.len(), 1);
            if file.path == "b.txt" {
                assert_eq!(hunks[0].lines[1].kind, DiffLineKind::Add);
                assert_eq!(file.status.kind, corvene_models::FileStatusKind::Untracked);
            }
        }
    }

    #[test]
    fn mode_only_change_is_a_text_diff_without_hunks() {
        let raw = ":100644 100755 aaa aaa M\0a.sh\0\ndiff --git a/a.sh b/a.sh\nold mode 100644\nnew mode 100755\n";
        let Diff::Text { hunks, warnings } = parse_raw_diff(raw.as_bytes()) else {
            panic!("text diff expected")
        };
        assert!(hunks.is_empty());
        assert_eq!(
            warnings.mode_change,
            Some(("100644".to_string(), "100755".to_string()))
        );
        let with_hunks = SAMPLE.replacen(
            "index 1..2 100644\n",
            "old mode 100644\nnew mode 100755\n",
            1,
        );
        let Diff::Text { hunks, warnings } = parse_unified(&with_hunks) else {
            panic!("text diff expected")
        };
        assert_eq!(hunks.len(), 1);
        assert!(warnings.mode_change.is_some());
    }

    #[test]
    fn type_change_sections_are_not_content() {
        let patch = "diff --git a/l b/l\ndeleted file mode 100644\nindex 1..0\n--- a/l\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\ndiff --git a/l b/l\nnew file mode 120000\nindex 0..2\n--- /dev/null\n+++ b/l\n@@ -0,0 +1 @@\n+target\n\\ No newline at end of file\n";
        let Diff::Text { hunks, warnings } = parse_unified(patch) else {
            panic!("text diff expected")
        };
        assert_eq!(hunks.len(), 2);
        let texts: Vec<_> = hunks
            .iter()
            .flat_map(|h| &h.lines[1..])
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        assert_eq!(
            texts,
            [
                (DiffLineKind::Delete, "one"),
                (DiffLineKind::Delete, "two"),
                (DiffLineKind::Add, "target"),
            ]
        );
        assert_eq!(hunks[1].unified_diff_start, 3);
        assert_eq!(hunks[1].lines[1].new_line, Some(1));
        assert_eq!(
            warnings.type_change,
            Some(("100644".to_string(), "120000".to_string()))
        );
        // one section: a plain new file is no type change
        let Diff::Text { warnings, .. } = parse_unified(
            "diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1 @@\n+a\n",
        ) else {
            panic!("text diff expected")
        };
        assert_eq!(warnings.type_change, None);
    }

    #[test]
    #[cfg_attr(not(unix), allow(unreachable_code))]
    fn symlink_lines_are_the_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("real.txt"), "one\ntwo\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("real.txt", dir.path().join("link")).unwrap();
        #[cfg(not(unix))]
        return;
        assert_eq!(
            working_file_lines(dir.path(), "link", false),
            Some(vec!["one".to_string(), "two".to_string()])
        );
        assert_eq!(
            working_file_lines(dir.path(), "link", true),
            Some(vec!["real.txt".to_string()])
        );
        assert_eq!(
            working_file_lines(dir.path(), "real.txt", true).map(|l| l.len()),
            Some(2)
        );
    }

    #[test]
    fn binary_file_as_text() {
        use std::process::Command;
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
        std::fs::write(path.join("data.bin"), b"one\0\ntwo\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
        std::fs::write(path.join("data.bin"), b"one\0\nTWO\n").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        let status = crate::get_status(git.clone(), path).unwrap();
        let file = &status.files[0];
        let binary =
            working_directory_diff(git.clone(), path, file, false, false, false, None).unwrap();
        assert_eq!(binary, Diff::Binary);
        let text = working_directory_diff(git, path, file, false, false, true, None).unwrap();
        assert!(matches!(text, Diff::Text { .. }));
    }

    #[test]
    fn parses_line_endings_warnings() {
        let old = "warning: CRLF will be replaced by LF in a.txt.\nThe file will have its original line endings in your working directory\n";
        assert_eq!(
            parse_line_endings_warning(old),
            Some(LineEndingsChange {
                from: "CRLF".into(),
                to: "LF".into()
            })
        );
        let new = "warning: in the working copy of 'a.txt', CRLF will be replaced by LF the next time Git touches it\n";
        assert_eq!(
            parse_line_endings_warning(new).map(|c| c.to),
            Some("LF".to_string())
        );
        assert_eq!(parse_line_endings_warning("warning: something else"), None);
        assert!(has_hidden_bidi_chars("abc\u{202E}def"));
        assert!(!has_hidden_bidi_chars("plain"));
    }
}
