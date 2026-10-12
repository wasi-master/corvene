//! Corvene `1306-utf16-diffs`: text files saved as UTF-16 (MetaEditor's
//! `.mq5`/`.mqh`, Windows `.reg` and `.rc` files, PowerShell scripts) are
//! diffed as text. Their NUL bytes make git take them for binary, so GHD
//! shows "This binary file has changed." (`lib/diff-parser.ts`, `lib/git/
//! diff.ts` `buildDiff`) and cannot commit or discard some of their lines.
//!
//! A file is UTF-16 when it starts with a byte order mark (`FF FE`, `FE
//! FF`) or, without one, when the zero bytes sit almost all on one side of
//! each pair (the high byte of ASCII text), and it decodes cleanly into
//! text with few control characters. Like git's own binary check, only a
//! file with a zero byte in its first 8000 bytes counts.
//!
//! The diff is git's (`git diff --no-index`) of the decoded text of both
//! sides, so hunks, line numbers and hunk expansion line up with the file
//! readers ([`crate::text_encoding::file_text`] decodes the same way). A
//! line selection is applied to the decoded text (the old side plus the
//! selected changes for the index, the working copy minus them for
//! Discard), each line keeping its own line ending, and written back as
//! UTF-16 with the file's byte order and byte order mark: `git apply`
//! cannot take a patch of the decoded text.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{
    Diff, DiffHunk, DiffLineKind, DiffSelection, Utf16Diff, Utf16Format, WorkingDirectoryFileChange,
};

use crate::detect::GitBinary;
use crate::error::{GitError, Result};
use crate::process::GitCommand;

/// Set by the dispatcher from the flag.
static DECODE_UTF16: AtomicBool = AtomicBool::new(false);

/// Turn UTF-16 text diffs on or off (flag `1306-utf16-diffs`).
pub fn set_decode_utf16(enabled: bool) {
    DECODE_UTF16.store(enabled, Ordering::Relaxed);
}

pub(crate) fn decode_utf16() -> bool {
    DECODE_UTF16.load(Ordering::Relaxed)
}

/// Larger files stay binary.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

/// How far git looks for a zero byte to call a file binary
/// (`buffer_is_binary`, `FIRST_FEW_BYTES`).
const FIRST_FEW_BYTES: usize = 8000;

/// How `bytes` are encoded when they are UTF-16 text ([`decode`]).
pub fn detect(bytes: &[u8]) -> Option<Utf16Format> {
    decode(bytes).map(|(format, _)| format)
}

/// `bytes` as UTF-16 text: its format and the text without the byte order
/// mark. `None` when they are not UTF-16 text (see the module docs).
pub fn decode(bytes: &[u8]) -> Option<(Utf16Format, String)> {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) || bytes.len() > MAX_BYTES {
        return None;
    }
    if !bytes[..bytes.len().min(FIRST_FEW_BYTES)].contains(&0) {
        return None;
    }
    let format = match bytes {
        // UTF-32LE's byte order mark
        [0xFF, 0xFE, 0, 0, ..] => return None,
        [0xFF, 0xFE, ..] => Utf16Format {
            big_endian: false,
            bom: true,
        },
        [0xFE, 0xFF, ..] => Utf16Format {
            big_endian: true,
            bom: true,
        },
        _ => without_bom(bytes)?,
    };
    let text = decode_as(bytes, format)?;
    plausible(&text).then_some((format, text))
}

/// The byte order of UTF-16 without a byte order mark: in mostly ASCII
/// text the high byte of at least half the code units is zero and the low
/// byte of almost none.
fn without_bom(bytes: &[u8]) -> Option<Utf16Format> {
    let units = bytes.len() / 2;
    let (mut even, mut odd) = (0usize, 0usize);
    for [first, second] in bytes.as_chunks::<2>().0 {
        even += usize::from(*first == 0);
        odd += usize::from(*second == 0);
    }
    let (high, low, big_endian) = if odd >= even {
        (odd, even, false)
    } else {
        (even, odd, true)
    };
    (high * 2 >= units && low * 50 <= units).then_some(Utf16Format {
        big_endian,
        bom: false,
    })
}

/// Text has at most 1 % control characters (tab, line breaks, form feed
/// and escape aside) and no noncharacters U+FFFE / U+FFFF.
fn plausible(text: &str) -> bool {
    let (mut chars, mut odd) = (0usize, 0usize);
    for c in text.chars() {
        chars += 1;
        let bad = match c {
            '\t' | '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1b}' => false,
            '\u{fffe}' | '\u{ffff}' => true,
            c => c.is_control(),
        };
        odd += usize::from(bad);
    }
    odd * 100 <= chars
}

/// `bytes` decoded as `format` (the byte order mark, which must be there
/// when `format` has one, left out); `None` when they are not.
pub fn decode_as(bytes: &[u8], format: Utf16Format) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let body = if format.bom {
        let mark: [u8; 2] = if format.big_endian {
            [0xFE, 0xFF]
        } else {
            [0xFF, 0xFE]
        };
        bytes.strip_prefix(&mark[..])?
    } else {
        bytes
    };
    let units = body.as_chunks::<2>().0.iter().map(|pair| {
        if format.big_endian {
            u16::from_be_bytes(*pair)
        } else {
            u16::from_le_bytes(*pair)
        }
    });
    char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .ok()
}

/// `text` encoded as `format`, with its byte order mark when it has one.
pub fn encode(text: &str, format: Utf16Format) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2 + 2);
    let mut push = |unit: u16| {
        out.extend_from_slice(&if format.big_endian {
            unit.to_be_bytes()
        } else {
            unit.to_le_bytes()
        })
    };
    if format.bom {
        push(0xFEFF);
    }
    text.encode_utf16().for_each(&mut push);
    out
}

/// A whole file's bytes as UTF-16 text while the flag is on (for
/// [`crate::text_encoding::file_text`]).
pub(crate) fn file_text(bytes: &[u8]) -> Option<String> {
    decode_utf16()
        .then(|| decode(bytes))
        .flatten()
        .map(|(_, text)| text)
}

/// One side of a file for [`text_diff`].
pub(crate) enum Side<'a> {
    /// The file has no such side (it is new, or deleted).
    Missing,
    /// The working copy at this path.
    File(&'a Path),
    /// The blob at a revision (empty for the index) and path.
    Blob(&'a str, &'a str),
}

/// A side's bytes: `Some(None)` for a missing side, `None` when it cannot
/// be read or is larger than [`MAX_BYTES`] (checked before reading).
fn read_side(git: Arc<GitBinary>, workdir: &Path, side: Side) -> Option<Option<Vec<u8>>> {
    let max = MAX_BYTES as u64;
    let bytes = match side {
        Side::Missing => return Some(None),
        Side::File(path) => {
            // a regular file only: never read through a link (a FIFO hangs)
            let meta = std::fs::symlink_metadata(path).ok()?;
            if !meta.is_file() || meta.len() > max {
                return None;
            }
            std::fs::read(path).ok()?
        }
        Side::Blob(rev, path) => {
            let spec = format!("{rev}:{path}");
            let in_process = crate::handle::open(workdir).ok().and_then(|repo| {
                let id = repo.rev_parse_single(spec.as_str()).ok()?;
                let header = id.header().ok()?;
                Some(
                    (header.kind() == gix::object::Kind::Blob && header.size() <= max)
                        .then(|| id.object().ok().map(|object| object.detach().data))
                        .flatten(),
                )
            });
            match in_process {
                Some(bytes) => bytes?,
                None => {
                    let size = GitCommand::new(git.clone())
                        .args(["cat-file", "-s", &spec])
                        .current_dir(workdir)
                        .run()
                        .ok()?
                        .stdout_string()
                        .ok()?;
                    if size.trim().parse::<u64>().ok()? > max {
                        return None;
                    }
                    crate::diff::blob_bytes(git, workdir, rev, path).ok()?
                }
            }
        }
    };
    Some(Some(bytes))
}

/// The diff of a file git took for binary as UTF-16 text, while the flag is
/// on: `new` and `old` are its sides (the new one is read first, so a file
/// that is not UTF-16 costs one read), `old_source` the blob the old side
/// is, `patch` git's binary patch (for its mode lines). `None` when a side
/// is not UTF-16 text or the two are encoded differently, so the diff stays
/// binary.
pub(crate) fn text_diff(
    git: Arc<GitBinary>,
    workdir: &Path,
    new: Side,
    old: Side,
    hide_whitespace: bool,
    patch: &[u8],
    old_source: Option<String>,
) -> Option<Diff> {
    if !decode_utf16() || (matches!(new, Side::Missing) && matches!(old, Side::Missing)) {
        return None;
    }
    let mut format = None;
    let mut side = |side: Side| -> Option<Option<String>> {
        let Some(bytes) = read_side(git.clone(), workdir, side)? else {
            return Some(None);
        };
        if bytes.is_empty() {
            return Some(Some(String::new()));
        }
        let (found, text) = decode(&bytes)?;
        if format.is_some_and(|f| f != found) {
            return None;
        }
        format = Some(found);
        Some(Some(text))
    };
    let new_text = side(new)?;
    let old_text = side(old)?;
    let format = format?;
    let diff = crate::diff::no_index_diff(
        git,
        old_text.as_deref().map(str::as_bytes),
        new_text.as_deref().map(str::as_bytes),
        hide_whitespace,
    )?;
    let mode_change = header_modes(patch);
    let marker = Utf16Diff { format, old_source };
    match diff {
        Diff::Text {
            hunks,
            mut warnings,
        } => {
            warnings.utf16 = Some(marker);
            warnings.mode_change = mode_change;
            Some(Diff::Text { hunks, warnings })
        }
        Diff::LargeText {
            hunks,
            mut warnings,
        } => {
            warnings.utf16 = Some(marker);
            warnings.mode_change = mode_change;
            Some(Diff::LargeText { hunks, warnings })
        }
        // only whitespace changed (`-w`), or only the mode
        Diff::Empty if mode_change.is_some() => Some(Diff::Text {
            hunks: Vec::new(),
            warnings: corvene_models::DiffWarnings {
                mode_change,
                utf16: Some(marker),
                ..Default::default()
            },
        }),
        Diff::Empty if hide_whitespace => Some(Diff::Empty),
        _ => None,
    }
}

/// The `old mode` / `new mode` lines of a binary patch's header.
fn header_modes(patch: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8_lossy(patch);
    let start = text.find("diff --git")?;
    let (mut old, mut new) = (None, None);
    for line in text[start..].split('\n') {
        if let Some(mode) = line.strip_prefix("old mode ") {
            old = Some(mode.trim().to_string());
        } else if let Some(mode) = line.strip_prefix("new mode ") {
            new = Some(mode.trim().to_string());
        } else if line.starts_with("Binary files ") || line.starts_with("@@") {
            break;
        }
    }
    old.zip(new)
}

/// `text`'s lines with their line endings.
fn lines_of(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Whether `line` (with its line ending) is the diff line `text` (the
/// parser drops the `\n` and one `\r`).
fn is_line(line: &str, text: &str) -> bool {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line) == text
}

/// The line ending most of `text`'s lines have.
fn line_ending(text: &str) -> &'static str {
    let lf = text.matches('\n').count();
    let crlf = text.matches("\r\n").count();
    if lf > 0 && crlf * 2 >= lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Lines put together; a line that had none (the old last line) gets a line
/// ending when another follows it.
struct Joined {
    text: String,
    ending: &'static str,
}

impl Joined {
    fn push(&mut self, line: &str) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push_str(self.ending);
        }
        self.text.push_str(line);
    }

    fn extend<'a>(&mut self, lines: impl IntoIterator<Item = &'a str>) {
        for line in lines {
            self.push(line);
        }
    }
}

/// A diff line's 0-based line on one side.
fn index(line: Option<u32>) -> Option<usize> {
    (line? as usize).checked_sub(1)
}

/// Where a hunk's lines start (0-based) on a side where it starts at
/// `start` with `count` lines: with no lines, `start` names the line
/// before.
fn hunk_first(start: u32, count: u32) -> usize {
    if count == 0 {
        start as usize
    } else {
        (start as usize).saturating_sub(1)
    }
}

/// The diff no longer matches the file.
#[derive(Debug, PartialEq, Eq)]
pub struct Mismatch;

/// `old` with the changes of `hunks` (the diff of `old` to `new`) that
/// `selection` selects: what `git apply --cached` of GHD's partial patch
/// (`formatPatch`) makes of it. Unselected additions are left out,
/// unselected deletions stay.
pub fn with_selected_changes(
    old: &str,
    new: &str,
    hunks: &[DiffHunk],
    selection: &DiffSelection,
) -> std::result::Result<String, Mismatch> {
    let (old_lines, new_lines) = (lines_of(old), lines_of(new));
    let mut out = Joined {
        text: String::with_capacity(old.len() + new.len() / 4),
        ending: line_ending(if old.is_empty() { new } else { old }),
    };
    let mut next = 0usize;
    let copy_to =
        |out: &mut Joined, next: &mut usize, to: usize| -> std::result::Result<(), Mismatch> {
            let lines = old_lines.get(*next..to).ok_or(Mismatch)?;
            out.extend(lines.iter().copied());
            *next = to;
            Ok(())
        };
    for hunk in hunks {
        copy_to(
            &mut out,
            &mut next,
            hunk_first(hunk.old_start, hunk.old_lines),
        )?;
        for (i, line) in hunk.lines.iter().enumerate() {
            let selected = selection.is_selected(hunk.unified_diff_start + i as u32);
            match line.kind {
                DiffLineKind::Hunk => {}
                DiffLineKind::Context | DiffLineKind::Delete => {
                    let at = index(line.old_line).ok_or(Mismatch)?;
                    copy_to(&mut out, &mut next, at)?;
                    let old_line = *old_lines.get(at).ok_or(Mismatch)?;
                    if line.kind == DiffLineKind::Delete && !is_line(old_line, &line.text) {
                        return Err(Mismatch);
                    }
                    if line.kind == DiffLineKind::Context || !selected {
                        out.push(old_line);
                    }
                    next = at + 1;
                }
                DiffLineKind::Add => {
                    let new_line = *new_lines
                        .get(index(line.new_line).ok_or(Mismatch)?)
                        .ok_or(Mismatch)?;
                    if !is_line(new_line, &line.text) {
                        return Err(Mismatch);
                    }
                    if selected {
                        out.push(new_line);
                    }
                }
            }
        }
    }
    copy_to(&mut out, &mut next, old_lines.len())?;
    Ok(out.text)
}

/// `new` without the changes of `hunks` (the diff of `old` to `new`) that
/// `selection` selects: what GHD's Discard patch
/// (`formatPatchToDiscardChanges`) makes of the working copy. Selected
/// additions go, selected deletions come back.
pub fn without_selected_changes(
    old: &str,
    new: &str,
    hunks: &[DiffHunk],
    selection: &DiffSelection,
) -> std::result::Result<String, Mismatch> {
    let (old_lines, new_lines) = (lines_of(old), lines_of(new));
    let mut out = Joined {
        text: String::with_capacity(new.len()),
        ending: line_ending(if new.is_empty() { old } else { new }),
    };
    let mut next = 0usize;
    let copy_to =
        |out: &mut Joined, next: &mut usize, to: usize| -> std::result::Result<(), Mismatch> {
            let lines = new_lines.get(*next..to).ok_or(Mismatch)?;
            out.extend(lines.iter().copied());
            *next = to;
            Ok(())
        };
    for hunk in hunks {
        copy_to(
            &mut out,
            &mut next,
            hunk_first(hunk.new_start, hunk.new_lines),
        )?;
        for (i, line) in hunk.lines.iter().enumerate() {
            let selected = selection.is_selected(hunk.unified_diff_start + i as u32);
            match line.kind {
                DiffLineKind::Hunk => {}
                DiffLineKind::Context | DiffLineKind::Add => {
                    let at = index(line.new_line).ok_or(Mismatch)?;
                    copy_to(&mut out, &mut next, at)?;
                    let new_line = *new_lines.get(at).ok_or(Mismatch)?;
                    if line.kind == DiffLineKind::Add && !is_line(new_line, &line.text) {
                        return Err(Mismatch);
                    }
                    if line.kind == DiffLineKind::Context || !selected {
                        out.push(new_line);
                    }
                    next = at + 1;
                }
                DiffLineKind::Delete => {
                    let old_line = *old_lines
                        .get(index(line.old_line).ok_or(Mismatch)?)
                        .ok_or(Mismatch)?;
                    if !is_line(old_line, &line.text) {
                        return Err(Mismatch);
                    }
                    if selected {
                        out.push(old_line);
                    }
                }
            }
        }
    }
    copy_to(&mut out, &mut next, new_lines.len())?;
    Ok(out.text)
}

/// What a line selection makes of a UTF-16 file, encoded like it.
pub(crate) struct Selected {
    /// The old side with the selected changes (the index, a stash).
    pub with: Vec<u8>,
    /// The working copy without them (Discard).
    pub without: Vec<u8>,
}

/// [`with_selected_changes`] and [`without_selected_changes`] of the
/// working copy of `path`, whose working diff is `hunks` / `info`. `None`
/// when no change is selected.
pub(crate) fn apply_selection(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    hunks: &[DiffHunk],
    selection: &DiffSelection,
    info: &Utf16Diff,
) -> Result<Option<Selected>> {
    let any_selected = hunks.iter().any(|hunk| {
        hunk.lines.iter().enumerate().any(|(i, line)| {
            matches!(line.kind, DiffLineKind::Add | DiffLineKind::Delete)
                && selection.is_selected(hunk.unified_diff_start + i as u32)
        })
    });
    if !any_selected {
        return Ok(None);
    }
    let changed = || {
        GitError::Gix(format!(
            "{path} changed since its diff was shown. Look at the changes again and retry."
        ))
    };
    let old_bytes = match &info.old_source {
        Some(spec) => {
            let (rev, old_path) = spec.split_once(':').ok_or_else(changed)?;
            crate::diff::blob_bytes(git, workdir, rev, old_path)?
        }
        None => Vec::new(),
    };
    let new_bytes = std::fs::read(workdir.join(path))?;
    let text = |bytes: &[u8]| -> Result<String> {
        if bytes.is_empty() {
            return Ok(String::new());
        }
        decode_as(bytes, info.format).ok_or_else(changed)
    };
    let (old, new) = (text(&old_bytes)?, text(&new_bytes)?);
    let with = with_selected_changes(&old, &new, hunks, selection).map_err(|_| changed())?;
    let without = without_selected_changes(&old, &new, hunks, selection).map_err(|_| changed())?;
    Ok(Some(Selected {
        with: encode(&with, info.format),
        without: encode(&without, info.format),
    }))
}

/// Put `bytes` into the index (`index_file`, else the repository's) as
/// `path`, keeping the entry's mode (a new entry takes the working file's):
/// `hash-object -w --stdin` (no filters), `update-index --cacheinfo`.
pub(crate) fn add_to_index(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    bytes: Vec<u8>,
    index_file: Option<&Path>,
) -> Result<()> {
    let command = || {
        let cmd = GitCommand::new(git.clone()).current_dir(workdir);
        match index_file {
            Some(index) => cmd.env("GIT_INDEX_FILE", index),
            None => cmd,
        }
    };
    let listed = command()
        .args(["ls-files", "-s", "-z", "--"])
        .arg(path)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .run()?;
    let mode = String::from_utf8_lossy(&listed.stdout)
        .split(' ')
        .next()
        .filter(|mode| mode.len() == 6 && mode.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
        .unwrap_or_else(|| working_file_mode(&workdir.join(path)).to_string());
    let oid = command()
        .args(["hash-object", "-w", "--stdin"])
        .stdin(bytes)
        .run()?
        .stdout_string()?
        .trim()
        .to_string();
    command()
        .args(["update-index", "--add", "--cacheinfo"])
        .arg(format!("{mode},{oid},{path}"))
        .run()?;
    Ok(())
}

/// The mode `git add` gives the file at `path`.
fn working_file_mode(path: &Path) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0) {
            return "100755";
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    "100644"
}

/// Stage the selected lines of a UTF-16 `file` whose working diff is
/// `hunks` / `info` (the partial patch of [`crate::patch`]).
pub(crate) fn stage_selection(
    git: Arc<GitBinary>,
    workdir: &Path,
    file: &WorkingDirectoryFileChange,
    hunks: &[DiffHunk],
    info: &Utf16Diff,
) -> Result<()> {
    match apply_selection(
        git.clone(),
        workdir,
        &file.path,
        hunks,
        &file.selection,
        info,
    )? {
        Some(selected) => add_to_index(git, workdir, &file.path, selected.with, None),
        None => Ok(()),
    }
}

/// Discard the selected lines of a UTF-16 file from its working copy;
/// `diff` is the working diff the selection was made in (GHD
/// `discardChangesFromSelection`).
pub fn discard_selection(
    git: Arc<GitBinary>,
    workdir: &Path,
    path: &str,
    diff: &Diff,
    selection: &DiffSelection,
) -> Result<()> {
    let (Some(hunks), Some(info)) = (diff.hunks(), diff.warnings().and_then(|w| w.utf16.as_ref()))
    else {
        return Err(GitError::Gix(format!("{path} is not a UTF-16 text diff")));
    };
    if let Some(selected) = apply_selection(git, workdir, path, hunks, selection, info)? {
        std::fs::write(workdir.join(path), selected.without)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvene_models::{DiffSelectionType, FileStatusKind};
    use std::process::Command;

    const LE_BOM: Utf16Format = Utf16Format {
        big_endian: false,
        bom: true,
    };

    #[test]
    fn detects_utf16_with_and_without_a_byte_order_mark() {
        let text = "int OnInit()\r\n{\r\n   return(INIT_SUCCEEDED);\r\n}\r\n";
        let le = encode(text, LE_BOM);
        assert_eq!(&le[..4], b"\xff\xfei\0");
        assert_eq!(decode(&le), Some((LE_BOM, text.to_string())));
        let be_bom = Utf16Format {
            big_endian: true,
            bom: true,
        };
        let be = encode(text, be_bom);
        assert_eq!(&be[..4], b"\xfe\xff\0i");
        assert_eq!(detect(&be), Some(be_bom));
        for big_endian in [false, true] {
            let format = Utf16Format {
                big_endian,
                bom: false,
            };
            assert_eq!(detect(&encode(text, format)), Some(format));
        }
        // non-ASCII text survives
        let accents = "// Größe – café 日本\n";
        assert_eq!(
            decode(&encode(accents, LE_BOM)).map(|(_, t)| t).as_deref(),
            Some(accents)
        );
    }

    #[test]
    fn binary_files_are_not_utf16() {
        // UTF-8 text, and text without a zero byte, as git sees it
        assert_eq!(detect(b"plain text\n"), None);
        assert_eq!(detect(b"\xff\xfe\x41\x42"), None);
        // odd length, UTF-32's mark
        assert_eq!(detect(b"\xff\xfeA\0B"), None);
        assert_eq!(detect(b"\xff\xfe\0\0A\0\0\0"), None);
        // an executable-like blob: zero bytes on both sides
        let blob: Vec<u8> = (0..4096u32)
            .flat_map(|i| [(i % 7) as u8 * 3, 0, (i % 251) as u8, 0, 0, 0])
            .collect();
        assert_eq!(detect(&blob), None);
        // 16-bit samples: zero high bytes but control characters
        let samples: Vec<u8> = (0..4096u32)
            .flat_map(|i| [(i * 37 % 256) as u8, 0])
            .collect();
        assert_eq!(detect(&samples), None);
        // a mark followed by noise: unpaired surrogates
        let mut noise = vec![0xFF, 0xFE];
        noise.extend((0..2000u32).flat_map(|i| (i.wrapping_mul(2_654_435_761) >> 7).to_le_bytes()));
        assert_eq!(detect(&noise), None);
    }

    #[test]
    fn round_trips_byte_for_byte() {
        let text = "a\r\nb\nc\r\n\r\nlast";
        for format in [
            LE_BOM,
            Utf16Format {
                big_endian: true,
                bom: false,
            },
        ] {
            let bytes = encode(text, format);
            assert_eq!(decode_as(&bytes, format).as_deref(), Some(text));
            assert_eq!(encode(&decode_as(&bytes, format).unwrap(), format), bytes);
        }
        // a mark is required when the format has one
        assert_eq!(decode_as(b"a\0", LE_BOM), None);
    }

    fn hunks(diff: &str) -> Vec<DiffHunk> {
        crate::parse_unified(diff).hunks().unwrap().to_vec()
    }

    #[test]
    fn applies_a_selection_to_the_decoded_text() {
        let old = "one\r\ntwo\r\nthree\r\nfour\r\n";
        let new = "ONE\r\ntwo\r\nthree\r\nfour\r\nfive\r\n";
        // lines: 0 hunk, 1 del one, 2 add ONE, 3-5 ctx, 6 add five
        let hunks = hunks(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,5 @@\n-one\r\n+ONE\r\n two\r\n three\r\n four\r\n+five\r\n",
        );
        let first = DiffSelection::none().with_range(1, 2, true);
        assert_eq!(
            with_selected_changes(old, new, &hunks, &first).unwrap(),
            "ONE\r\ntwo\r\nthree\r\nfour\r\n"
        );
        assert_eq!(
            without_selected_changes(old, new, &hunks, &first).unwrap(),
            "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n"
        );
        let last = DiffSelection::none().with_line(6, true);
        assert_eq!(
            with_selected_changes(old, new, &hunks, &last).unwrap(),
            "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n"
        );
        // a file that changed since
        assert_eq!(
            with_selected_changes("uno\r\n", new, &hunks, &first),
            Err(Mismatch)
        );
    }

    #[test]
    fn a_missing_final_line_ending_is_added_before_more_lines() {
        let old = "a\r\nb";
        let new = "a\r\nb\r\nc\r\n";
        let hunks = hunks(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n a\r\n-b\n\\ No newline at end of file\n+b\r\n+c\r\n",
        );
        // only `c`: the old `b` stays and gets a line ending
        let c = DiffSelection::none().with_line(4, true);
        assert_eq!(
            with_selected_changes(old, new, &hunks, &c).unwrap(),
            "a\r\nb\r\nc\r\n"
        );
        assert_eq!(
            without_selected_changes(old, new, &hunks, &c).unwrap(),
            "a\r\nb\r\n"
        );
    }

    fn git(path: &Path, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        out.stdout
    }

    #[test]
    fn stages_commits_and_discards_lines_of_a_utf16_file() {
        set_decode_utf16(true);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "-q", "-b", "main"]);
        git(path, &["config", "commit.gpgsign", "false"]);
        git(path, &["config", "user.name", "T"]);
        git(path, &["config", "user.email", "t@example.com"]);
        let head = "//+--+\r\n#property strict\r\nint OnInit()\r\n{\r\n   return(0);\r\n}\r\n\r\nvoid OnTick()\r\n{\r\n   Print(\"tick\");\r\n}\r\n";
        std::fs::write(path.join("ea.mq5"), encode(head, LE_BOM)).unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-q", "-m", "init"]);
        let working = head
            .replace("return(0)", "return(INIT_SUCCEEDED)")
            .replace("Print(\"tick\")", "Print(\"tick: \", Symbol())");
        std::fs::write(path.join("ea.mq5"), encode(&working, LE_BOM)).unwrap();

        let gitbin = Arc::new(crate::find_git().unwrap());
        let mut status = crate::get_status(gitbin.clone(), path).unwrap();
        let diff = crate::working_directory_diff(
            gitbin.clone(),
            path,
            &status.files[0],
            false,
            false,
            false,
            None,
        )
        .unwrap();
        let info = diff.warnings().and_then(|w| w.utf16.clone()).unwrap();
        assert_eq!(info.format, LE_BOM);
        assert_eq!(info.old_source.as_deref(), Some(":ea.mq5"));
        let changed: Vec<(DiffLineKind, String)> = diff
            .hunks()
            .unwrap()
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| matches!(l.kind, DiffLineKind::Add | DiffLineKind::Delete))
            .map(|l| (l.kind, l.text.clone()))
            .collect();
        assert_eq!(
            changed,
            [
                (DiffLineKind::Delete, "   return(0);".to_string()),
                (DiffLineKind::Add, "   return(INIT_SUCCEEDED);".to_string()),
                (DiffLineKind::Delete, "   Print(\"tick\");".to_string()),
                (
                    DiffLineKind::Add,
                    "   Print(\"tick: \", Symbol());".to_string()
                ),
            ]
        );
        // the file readers decode the same way (hunk expansion)
        assert_eq!(
            crate::working_file_lines(path, "ea.mq5", true).unwrap()[4],
            "   return(INIT_SUCCEEDED);"
        );
        // the first change only, through the commit's staging
        let rows: Vec<u32> = diff
            .hunks()
            .unwrap()
            .iter()
            .flat_map(|h| {
                h.lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.text.contains("return"))
                    .map(move |(i, _)| h.unified_diff_start + i as u32)
            })
            .collect();
        status.files[0].selection = rows
            .iter()
            .fold(DiffSelection::none(), |s, row| s.with_line(*row, true));
        assert_eq!(status.files[0].selection.kind(), DiffSelectionType::Partial);
        crate::unstage_all(gitbin.clone(), path).unwrap();
        crate::stage_files(gitbin.clone(), path, &status.files).unwrap();
        crate::stage_partial_files(gitbin.clone(), path, &status.files).unwrap();
        let staged = head.replace("return(0)", "return(INIT_SUCCEEDED)");
        assert_eq!(git(path, &["show", ":ea.mq5"]), encode(&staged, LE_BOM));
        assert_eq!(git(path, &["ls-files", "-s", "ea.mq5"])[..6], b"100644"[..]);
        crate::commit(gitbin.clone(), path, "partial\n", &Default::default()).unwrap();
        assert_eq!(git(path, &["show", "HEAD:ea.mq5"]), encode(&staged, LE_BOM));

        // the History diff of that commit
        let files = crate::get_changed_files(gitbin.clone(), path, "HEAD", false).unwrap();
        let committed =
            crate::commit_file_diff(gitbin.clone(), path, &files.files[0], false).unwrap();
        assert!(committed.warnings().is_some_and(|w| w.utf16.is_some()));
        assert_eq!(committed.hunks().unwrap()[0].lines.len(), 9);

        // discard the other change from the working copy
        let status = crate::get_status(gitbin.clone(), path).unwrap();
        let diff = crate::working_directory_diff(
            gitbin.clone(),
            path,
            &status.files[0],
            false,
            false,
            false,
            None,
        )
        .unwrap();
        discard_selection(gitbin.clone(), path, "ea.mq5", &diff, &DiffSelection::all()).unwrap();
        assert_eq!(
            std::fs::read(path.join("ea.mq5")).unwrap(),
            encode(&staged, LE_BOM)
        );
        assert!(crate::get_status(gitbin, path).unwrap().files.is_empty());
    }

    #[test]
    fn a_new_utf16_file_is_staged_line_by_line() {
        set_decode_utf16(true);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        git(path, &["init", "-q", "-b", "main"]);
        let format = Utf16Format {
            big_endian: true,
            bom: false,
        };
        std::fs::write(path.join("new.txt"), encode("one\ntwo\nthree\n", format)).unwrap();
        let gitbin = Arc::new(crate::find_git().unwrap());
        let mut status = crate::get_status(gitbin.clone(), path).unwrap();
        assert_eq!(status.files[0].status.kind, FileStatusKind::Untracked);
        let diff = crate::working_directory_diff(
            gitbin.clone(),
            path,
            &status.files[0],
            false,
            false,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            diff.warnings().unwrap().utf16.as_ref().unwrap().format,
            format
        );
        // lines: 0 hunk, 1 one, 2 two, 3 three
        status.files[0].selection = DiffSelection::all().with_line(2, false);
        crate::stage_partial_files(gitbin, path, &status.files).unwrap();
        assert_eq!(
            git(path, &["show", ":new.txt"]),
            encode("one\nthree\n", format)
        );
    }
}
