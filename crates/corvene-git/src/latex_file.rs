//! Corvene `1332-latex-diff`: LaTeX documents (`.tex`, `.ltx`, `.latex`)
//! and BibTeX bibliographies (`.bib`, `.bibtex`) shown rendered next to
//! git's own diff, with the pictures a document's `\includegraphics`
//! names read from the repository at the same revision.
//!
//! GitHub Desktop shows these files as their text diff only
//! (`app/src/lib/git/diff.ts`). Corvene does not run TeX: the UI renders
//! the common markup itself, so only the text and the pictures are read
//! here. `.sty`, `.cls` and `.dtx` files are macro code and stay text.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{Diff, FileStatusKind, ImageBlob, LatexBlob, LatexKind};

pub use crate::linked_files::Side;
use crate::linked_files::{Budget, resolve};

/// Set by the dispatcher from the flag.
static READ_LATEX_FILES: AtomicBool = AtomicBool::new(false);

/// Turn LaTeX diffs on or off.
pub fn set_read_latex_files(enabled: bool) {
    READ_LATEX_FILES.store(enabled, Ordering::Relaxed);
}

fn read_latex_files() -> bool {
    READ_LATEX_FILES.load(Ordering::Relaxed)
}

/// A LaTeX document or BibTeX file while the flag is on.
pub fn is_latex(path: &str) -> bool {
    read_latex_files() && corvene_models::latex_kind(path).is_some()
}

/// Larger files keep git's diff only.
pub const MAX_BYTES: usize = 16 << 20;
/// Pictures read per side.
const MAX_IMAGES: usize = 32;
/// Picture bytes read per side.
const MAX_IMAGE_BYTES: usize = 16 << 20;
/// Extensions `\includegraphics` tries for a name without one, in
/// pdfLaTeX's order (EPS needs a converter and is not drawn).
const GRAPHICS_EXTENSIONS: [&str; 6] = [".pdf", ".png", ".jpg", ".jpeg", ".svg", ".eps"];

/// A change of a LaTeX or BibTeX file as a [`Diff::Latex`] next to git's
/// diff `source`; `None` when the flag is off, the path is not a LaTeX
/// file or a side is too large (the caller keeps `source`). `linked` reads
/// a repository file (by repository-relative path) at a side's revision,
/// for pictures.
pub fn latex_diff(
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
    source: Diff,
    linked: impl Fn(Side, &str) -> Option<Vec<u8>>,
) -> Option<Diff> {
    if !read_latex_files() {
        return None;
    }
    let latex_kind = corvene_models::latex_kind(path)?;
    // `796-lfs-text-diff` turns a pointer diff into the contents' diff
    if source.hunks().is_some_and(crate::lfs::is_pointer_diff) {
        return None;
    }
    let side = |which: Side, bytes: Option<Vec<u8>>| -> Option<Option<Arc<LatexBlob>>> {
        let Some(bytes) = bytes else {
            return Some(None);
        };
        if bytes.len() > MAX_BYTES {
            return None;
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
        let images = match latex_kind {
            LatexKind::Document => graphics(path, &text, |rel| linked(which, rel)),
            LatexKind::Bibliography => Vec::new(),
        };
        Some(Some(Arc::new(LatexBlob { text, images })))
    };
    let current = side(
        Side::Current,
        (kind != FileStatusKind::Deleted).then(current).flatten(),
    )?;
    let previous = side(
        Side::Previous,
        (!kind.is_new_or_untracked()).then(previous).flatten(),
    )?;
    if current.is_none() && previous.is_none() {
        return None;
    }
    Some(Diff::Latex {
        kind: latex_kind,
        previous,
        current,
        source: Box::new(source),
    })
}

/// The pictures a document's `\includegraphics` names, found relative to
/// the document's folder and each `\graphicspath` folder.
fn graphics(
    path: &str,
    text: &str,
    read: impl Fn(&str) -> Option<Vec<u8>>,
) -> Vec<(String, ImageBlob)> {
    let text = strip_comments(text);
    let dirs = graphics_paths(&text);
    let mut out: Vec<(String, ImageBlob)> = Vec::new();
    let mut budget = Budget {
        files: MAX_IMAGES,
        bytes: MAX_IMAGE_BYTES,
    };
    for name in included_graphics(&text) {
        if budget.files == 0 {
            break;
        }
        if out.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let found = candidates(path, &dirs, &name).into_iter().find_map(|rel| {
            let media_type = graphics_type(&rel)?;
            Some((read(&rel)?, media_type))
        });
        let Some((bytes, media_type)) = found else {
            continue;
        };
        if !budget.take(bytes.len()) {
            continue;
        }
        out.push((
            name,
            ImageBlob {
                bytes,
                media_type: media_type.to_string(),
                original: None,
            },
        ));
    }
    out
}

/// The media type of a picture file the diff can draw.
fn graphics_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => Some("application/pdf"),
        "eps" | "ps" => None,
        _ => crate::linked_files::image_type(path),
    }
}

/// The repository paths a picture name may be at, in the order TeX looks.
fn candidates(path: &str, dirs: &[String], name: &str) -> Vec<String> {
    let has_ext = name
        .rsplit('/')
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .is_some_and(|(stem, _)| !stem.is_empty());
    let mut out = Vec::new();
    let prefixes = std::iter::once(String::new()).chain(dirs.iter().map(|d| {
        if d.ends_with('/') {
            d.clone()
        } else {
            format!("{d}/")
        }
    }));
    for prefix in prefixes {
        let base = format!("{prefix}{name}");
        let Some(rel) = resolve(path, &base) else {
            continue;
        };
        if has_ext {
            out.push(rel.clone());
        }
        for ext in GRAPHICS_EXTENSIONS {
            out.push(format!("{rel}{ext}"));
        }
    }
    out.dedup();
    out
}

/// The text with `%` comments removed (`\%` is a percent sign).
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let bytes = line.as_bytes();
        let mut cut = None;
        let mut backslashes = 0usize;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'%' && backslashes.is_multiple_of(2) {
                cut = Some(i);
                break;
            }
            backslashes = if b == b'\\' { backslashes + 1 } else { 0 };
        }
        match cut {
            // a comment also swallows the line break
            Some(i) => out.push_str(&line[..i]),
            None => out.push_str(line),
        }
    }
    out
}

/// Positions just after each `\name` command (not `\namex`).
fn commands<'a>(text: &'a str, name: &'a str) -> impl Iterator<Item = usize> + 'a {
    let mut from = 0;
    std::iter::from_fn(move || {
        while let Some(at) = text[from..].find('\\') {
            let start = from + at;
            from = start + 1;
            let rest = &text[start + 1..];
            if rest.starts_with(name)
                && !rest[name.len()..]
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic() || b == b'@')
            {
                from = start + 1 + name.len();
                return Some(from);
            }
        }
        None
    })
}

fn skip_ws(text: &str, mut i: usize) -> usize {
    while text[i..].starts_with(|c: char| c.is_whitespace()) {
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    i
}

/// The contents of the brace group (`open` `{` … `}`, nested braces kept)
/// at `i` (after whitespace), and the position after it.
fn group(text: &str, i: usize, open: u8, close: u8) -> Option<(&str, usize)> {
    let i = skip_ws(text, i);
    let bytes = text.as_bytes();
    if bytes.get(i) != Some(&open) {
        return None;
    }
    let mut depth = 0usize;
    let mut j = i;
    while j < bytes.len() {
        let b = bytes[j];
        if b == b'\\' {
            j += 2;
            continue;
        }
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some((&text[i + 1..j], j + 1));
            }
        }
        j += 1;
    }
    None
}

/// The folders of every `\graphicspath{{dir1/}{dir2/}}`.
fn graphics_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for at in commands(text, "graphicspath") {
        let Some((inner, _)) = group(text, at, b'{', b'}') else {
            continue;
        };
        let mut i = 0;
        let mut any = false;
        while let Some((dir, next)) = group(inner, i, b'{', b'}') {
            any = true;
            let dir = dir.trim();
            if !dir.is_empty() && !out.iter().any(|d| d == dir) {
                out.push(dir.to_string());
            }
            i = next;
        }
        // `\graphicspath{dir/}` (one folder without inner braces)
        let inner = inner.trim();
        if !any && !inner.is_empty() && !out.iter().any(|d| d == inner) {
            out.push(inner.to_string());
        }
    }
    out
}

/// The names of every `\includegraphics[options]{name}`, as written.
fn included_graphics(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for at in commands(text, "includegraphics") {
        let mut i = at;
        if text[i..].starts_with('*') {
            i += 1;
        }
        // `[options]`, or the old `[llx,lly][urx,ury]`
        while let Some((_, next)) = group(text, i, b'[', b']') {
            i = next;
        }
        let Some((name, _)) = group(text, i, b'{', b'}') else {
            continue;
        };
        let name = name.trim();
        let name = name
            .strip_prefix('"')
            .and_then(|n| n.strip_suffix('"'))
            .unwrap_or(name);
        if name.is_empty() || name.contains('\\') || name.contains('#') {
            continue;
        }
        out.push(name.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_stripped() {
        assert_eq!(strip_comments("a % b\nc\n"), "a c\n");
        assert_eq!(strip_comments("50\\% off\n"), "50\\% off\n");
        assert_eq!(strip_comments("x\\\\% comment\ny"), "x\\\\y");
    }

    #[test]
    fn graphicspath_is_parsed() {
        let text = "\\graphicspath{ {figures/}{./img/} }\n\\graphicspath{plots/}";
        assert_eq!(graphics_paths(text), ["figures/", "./img/", "plots/"]);
        assert!(graphics_paths("\\graphicspathx{{a/}}").is_empty());
    }

    #[test]
    fn includegraphics_names_are_read() {
        let text = "\\includegraphics[width=0.5\\linewidth]{fig/a}\n\
                    \\includegraphics*[0,0][1,1]{ b.png }\n\
                    \\includegraphics{\"c d\"}\n\
                    \\includegraphics{\\macro}\n\
                    \\includegraphicsx{nope}";
        assert_eq!(included_graphics(text), ["fig/a", "b.png", "c d"]);
    }

    #[test]
    fn commented_includes_are_ignored() {
        let text = strip_comments("% \\includegraphics{old}\n\\includegraphics{new}\n");
        assert_eq!(included_graphics(&text), ["new"]);
    }

    #[test]
    fn candidates_follow_tex_lookup() {
        let dirs = vec!["figures/".to_string(), "../shared".to_string()];
        let c = candidates("paper/main.tex", &dirs, "plot");
        assert_eq!(c[0], "paper/plot.pdf");
        assert!(c.contains(&"paper/figures/plot.png".to_string()));
        assert!(c.contains(&"shared/plot.svg".to_string()));
        let c = candidates("main.tex", &[], "a.png");
        assert_eq!(c[0], "a.png");
        assert!(candidates("main.tex", &[], "../x").is_empty());
    }

    #[test]
    fn graphics_resolve_through_graphicspath() {
        let text = "\\graphicspath{{figures/}}\n\\includegraphics{plot}\n\
                    \\includegraphics{logo.eps}\n\\includegraphics{missing}";
        let images = graphics("doc/main.tex", text, |rel| {
            (rel == "doc/figures/plot.png").then(|| b"\x89PNG".to_vec())
        });
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].0, "plot");
        assert_eq!(images[0].1.media_type, "image/png");
    }

    #[test]
    fn diff_needs_the_flag() {
        set_read_latex_files(true);
        let source = Diff::Empty;
        let diff = latex_diff(
            "refs.bib",
            FileStatusKind::Modified,
            || Some(b"@book{a, title={New}}".to_vec()),
            || Some(b"@book{a, title={Old}}".to_vec()),
            source,
            |_, _| None,
        );
        let Some(Diff::Latex {
            kind,
            previous: Some(previous),
            current: Some(current),
            ..
        }) = diff
        else {
            panic!("{diff:?}");
        };
        assert_eq!(kind, LatexKind::Bibliography);
        assert!(previous.text.contains("Old"));
        assert!(current.text.contains("New"));
        assert!(
            latex_diff(
                "a.sty",
                FileStatusKind::Modified,
                || None,
                || None,
                Diff::Empty,
                |_, _| None
            )
            .is_none()
        );
    }
}
