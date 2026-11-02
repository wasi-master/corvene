//! Corvene `1117-browse-files` (desktop/desktop#13831): what the file browser
//! reads. GitHub Desktop has no file browser.

use std::path::Path;
use std::sync::Arc;

use crate::detect::GitBinary;
use crate::error::Result;
use crate::process::GitCommand;

fn nul_separated(stdout: &[u8]) -> Vec<String> {
    let mut paths: Vec<String> = stdout
        .split(|b| *b == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    paths.sort_unstable();
    paths.dedup();
    paths
}

/// Every file at `rev`, or in the working tree (tracked and untracked, not
/// ignored) for `None`, sorted.
pub fn list_files(git: Arc<GitBinary>, workdir: &Path, rev: Option<&str>) -> Result<Vec<String>> {
    let command = GitCommand::new(git).current_dir(workdir);
    let command = match rev {
        Some(rev) => command.args(["ls-tree", "-r", "-z", "--name-only", rev]),
        None => command.args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ]),
    };
    Ok(nul_separated(&command.run()?.stdout))
}

/// The text a quick edit starts from, and how to write it back unchanged
/// apart from the edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditableText {
    pub text: String,
    bom: bool,
    crlf: bool,
}

/// Why a file cannot be quick-edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotEditable {
    NotUtf8,
    MixedLineEndings,
}

/// UTF-8 files whose lines all end the same way. Anything else could not be
/// written back byte for byte apart from the edit, so it stays read-only.
pub fn editable_text(bytes: &[u8]) -> std::result::Result<EditableText, NotEditable> {
    const BOM: &[u8] = b"\xef\xbb\xbf";
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(body) => (true, body),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body).map_err(|_| NotEditable::NotUtf8)?;
    let newlines = text.matches('\n').count();
    let crlfs = text.matches("\r\n").count();
    if crlfs != 0 && crlfs != newlines {
        return Err(NotEditable::MixedLineEndings);
    }
    let crlf = crlfs != 0;
    let text = if crlf {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    };
    Ok(EditableText { text, bom, crlf })
}

impl EditableText {
    /// The file's bytes with `edited` as its text.
    pub fn encode(&self, edited: &str) -> Vec<u8> {
        let body = if self.crlf {
            edited.replace('\n', "\r\n")
        } else {
            edited.to_string()
        };
        let mut out = Vec::with_capacity(body.len() + 3);
        if self.bom {
            out.extend_from_slice(b"\xef\xbb\xbf");
        }
        out.extend_from_slice(body.as_bytes());
        out
    }
}

/// `bytes` with line `line` (1-based) replaced by `new`, its line end kept
/// (Corvene `1366-diff-line-edits`). `None` when that line no longer reads
/// `old`, so an edit never lands on a file that changed under it.
pub fn replace_line(bytes: &[u8], line: u32, old: &str, new: &str) -> Option<Vec<u8>> {
    let index = usize::try_from(line.checked_sub(1)?).ok()?;
    let mut out = Vec::with_capacity(bytes.len() + new.len());
    let mut found = false;
    for (ix, current) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        if ix != index {
            out.extend_from_slice(current);
            continue;
        }
        let (body, end) = match current.strip_suffix(b"\r\n") {
            Some(body) => (body, &b"\r\n"[..]),
            None => match current.strip_suffix(b"\n") {
                Some(body) => (body, &b"\n"[..]),
                None => (current, &b""[..]),
            },
        };
        if body != old.as_bytes() {
            return None;
        }
        out.extend_from_slice(new.as_bytes());
        out.extend_from_slice(end);
        found = true;
    }
    found.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_round_trip_byte_for_byte() {
        for bytes in [
            &b"a\nb\n"[..],
            b"a\nb",
            b"a\r\nb\r\n",
            b"\xef\xbb\xbfa\r\nb",
            b"",
        ] {
            let editable = editable_text(bytes).unwrap();
            assert!(!editable.text.contains('\r'));
            assert_eq!(editable.encode(&editable.text), bytes);
        }
        let crlf = editable_text(b"a\r\nb\r\n").unwrap();
        assert_eq!(crlf.encode("a\nB\nc\n"), b"a\r\nB\r\nc\r\n");
    }

    #[test]
    fn what_cannot_round_trip_is_not_editable() {
        assert_eq!(
            editable_text(b"a\r\nb\n"),
            Err(NotEditable::MixedLineEndings)
        );
        assert_eq!(editable_text(b"\xff\xfe"), Err(NotEditable::NotUtf8));
    }

    #[test]
    fn replacing_a_line_keeps_everything_else() {
        assert_eq!(
            replace_line(b"a\r\nb\nc", 1, "a", "A").unwrap(),
            b"A\r\nb\nc"
        );
        assert_eq!(replace_line(b"a\nb\nc", 3, "c", "C").unwrap(), b"a\nb\nC");
        assert_eq!(replace_line(b"a\nb\n", 2, "b", "").unwrap(), b"a\n\n");
        assert_eq!(replace_line(b"a\nb\n", 2, "other", "x"), None);
        assert_eq!(replace_line(b"a\n", 5, "a", "x"), None);
        assert_eq!(replace_line(b"a\n", 0, "a", "x"), None);
    }

    #[test]
    fn lists_working_tree_and_revision_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(["-c", "user.name=T", "-c", "user.email=t@t"])
                    .args(args)
                    .current_dir(path)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["init", "-q"]);
        std::fs::create_dir(path.join("d")).unwrap();
        std::fs::write(path.join("d/tracked"), "t").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "c"]);
        std::fs::write(path.join("untracked"), "u").unwrap();
        std::fs::write(path.join(".gitignore"), "ignored\n").unwrap();
        std::fs::write(path.join("ignored"), "i").unwrap();
        let git = Arc::new(crate::find_git().unwrap());
        assert_eq!(
            list_files(git.clone(), path, None).unwrap(),
            [".gitignore", "d/tracked", "untracked"]
        );
        assert_eq!(list_files(git, path, Some("HEAD")).unwrap(), ["d/tracked"]);
    }
}
