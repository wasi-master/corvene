//! Repository files a text file links to by relative path (a notebook's
//! markdown images, Corvene `1331-notebook-diff`; a LaTeX document's
//! `\includegraphics`, Corvene `1332-latex-diff`), read at the revision of
//! the side that links them. GitHub Desktop shows both file types as text
//! and reads no linked files.

use std::path::Path;

/// The side of a diff a linked file is read for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The old side (the commit's parent, `HEAD` or the index).
    Previous,
    /// The new side (the working tree or the commit).
    Current,
}

/// `link` relative to the folder of the repository file `from`
/// (`nb/a.ipynb` + `../img/x.png` → `img/x.png`). `None` for an absolute
/// path, a URL or a path that leaves the repository.
pub(crate) fn resolve(from: &str, link: &str) -> Option<String> {
    if link.is_empty() || link.starts_with('/') || link.starts_with('\\') || has_scheme(link) {
        return None;
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    parts.retain(|p| !p.is_empty() && *p != ".");
    for seg in link.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            seg => parts.push(seg),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `http:`, `data:`, `attachment:`, … (a Windows drive letter counts too).
pub(crate) fn has_scheme(link: &str) -> bool {
    let Some((scheme, _)) = link.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `%20` and other escapes of a link decoded.
pub(crate) fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && hex.iter().all(u8::is_ascii_hexdigit)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The media type of an image file a diff can draw (`.svg` included).
pub(crate) fn image_type(path: &str) -> Option<&'static str> {
    corvene_models::image_media_type(path).or_else(|| {
        path.rsplit_once('.')
            .is_some_and(|(_, e)| e.eq_ignore_ascii_case("svg"))
            .then_some("image/svg+xml")
    })
}

/// A regular file of the working tree (not a symbolic link, which git
/// stores as its target path) of at most `max` bytes.
pub(crate) fn read_working(workdir: &Path, rel: &str, max: usize) -> Option<Vec<u8>> {
    let path = workdir.join(rel);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > max as u64 {
        return None;
    }
    std::fs::read(path).ok()
}

/// A blob of `rev` (`""` for the index) of at most `max` bytes, read in
/// process only: a missing file is the common case and must not cost a
/// `git show` spawn per guess.
pub(crate) fn read_blob(workdir: &Path, rev: &str, rel: &str, max: usize) -> Option<Vec<u8>> {
    crate::handle::blob_bytes(workdir, rev, rel).filter(|b| b.len() <= max)
}

/// Collects linked files within a count and byte budget.
pub(crate) struct Budget {
    pub(crate) files: usize,
    pub(crate) bytes: usize,
}

impl Budget {
    /// Take `len` bytes for one more file; `false` when over the budget.
    pub(crate) fn take(&mut self, len: usize) -> bool {
        if self.files == 0 || len > self.bytes {
            return false;
        }
        self.files -= 1;
        self.bytes -= len;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_links() {
        assert_eq!(resolve("a.ipynb", "x.png").as_deref(), Some("x.png"));
        assert_eq!(
            resolve("nb/a.ipynb", "../img/x.png").as_deref(),
            Some("img/x.png")
        );
        assert_eq!(
            resolve("nb/a.ipynb", "./img/./x.png").as_deref(),
            Some("nb/img/x.png")
        );
        assert_eq!(resolve("nb/a.ipynb", "../../x.png"), None);
        assert_eq!(resolve("a.ipynb", "/etc/x.png"), None);
        assert_eq!(resolve("a.ipynb", "https://x/y.png"), None);
        assert_eq!(resolve("a.ipynb", "attachment:y.png"), None);
        assert_eq!(resolve("a.ipynb", "data:image/png;base64,AA"), None);
    }

    #[test]
    fn decodes_escapes() {
        assert_eq!(percent_decode("a%20b.png"), "a b.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("a%2"), "a%2");
    }
}
