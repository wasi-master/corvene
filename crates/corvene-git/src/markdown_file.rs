//! Corvene `1334-markdown-preview-diff`: Markdown files (and MDX, R
//! Markdown, Quarto, Livebook) read for a rendered diff next to git's own
//! diff, with the repository pictures they link to (`![alt](img.png)`,
//! `[id]: img.png`, `<img src>`) read at the same revision.
//!
//! GitHub Desktop shows Markdown as its text diff only
//! (`app/src/lib/git/diff.ts`); github.com offers a "rich diff" of the
//! rendered document beside the source diff.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{Diff, FileStatusKind, ImageBlob, MarkdownBlob};

pub use crate::linked_files::Side;
use crate::linked_files::{Budget, image_type, percent_decode, resolve};

/// Set by the dispatcher from the flag.
static READ_MARKDOWN_FILES: AtomicBool = AtomicBool::new(false);

/// Turn Markdown previews on or off.
pub fn set_read_markdown_files(enabled: bool) {
    READ_MARKDOWN_FILES.store(enabled, Ordering::Relaxed);
}

/// A Markdown file while the flag is on.
pub fn is_markdown(path: &str) -> bool {
    READ_MARKDOWN_FILES.load(Ordering::Relaxed) && corvene_models::is_markdown_file(path)
}

/// Larger files keep git's diff only.
pub const MAX_BYTES: usize = 8 << 20;
/// Pictures read per side.
const MAX_IMAGES: usize = 48;
/// Picture bytes read per side.
const MAX_IMAGE_BYTES: usize = 24 << 20;

/// A change of a Markdown file as a [`Diff::Markdown`] next to git's diff
/// `source`; `None` when the flag is off, the path is not Markdown or a
/// side is too large (the caller keeps `source`). `linked` reads a
/// repository file (by repository-relative path) at a side's revision.
pub fn markdown_diff(
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
    source: Diff,
    linked: impl Fn(Side, &str) -> Option<Vec<u8>>,
) -> Option<Diff> {
    if !is_markdown(path) {
        return None;
    }
    // `796-lfs-text-diff` turns a pointer diff into the contents' diff
    if source.hunks().is_some_and(crate::lfs::is_pointer_diff) {
        return None;
    }
    let side = |which: Side, bytes: Option<Vec<u8>>| -> Option<Option<Arc<MarkdownBlob>>> {
        let Some(bytes) = bytes else {
            return Some(None);
        };
        if bytes.len() > MAX_BYTES {
            return None;
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
        let images = pictures(path, &text, |rel| linked(which, rel));
        Some(Some(Arc::new(MarkdownBlob { text, images })))
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
    Some(Diff::Markdown {
        previous,
        current,
        source: Box::new(source),
    })
}

/// The repository pictures the document links to, by the source as
/// written.
fn pictures(
    path: &str,
    text: &str,
    read: impl Fn(&str) -> Option<Vec<u8>>,
) -> Vec<(String, ImageBlob)> {
    let mut out: Vec<(String, ImageBlob)> = Vec::new();
    let mut budget = Budget {
        files: MAX_IMAGES,
        bytes: MAX_IMAGE_BYTES,
    };
    for src in image_sources(text) {
        if budget.files == 0 {
            break;
        }
        if out.iter().any(|(s, _)| *s == src) {
            continue;
        }
        let link = percent_decode(src.split(['#', '?']).next().unwrap_or(&src));
        let Some(rel) = resolve(path, &link) else {
            continue;
        };
        let Some(media_type) = image_type(&rel) else {
            continue;
        };
        let Some(bytes) = read(&rel) else {
            continue;
        };
        if !budget.take(bytes.len()) {
            continue;
        }
        out.push((
            src,
            ImageBlob {
                bytes,
                media_type: media_type.to_string(),
                original: None,
            },
        ));
    }
    out
}

/// Picture sources outside code: `![alt](src "title")`, `![alt](<src>)`,
/// reference definitions (`[id]: src`) and HTML `<img src>`.
fn image_sources(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        if let Some(c) = marker {
            let n = trimmed.chars().take_while(|x| *x == c).count();
            if n >= 3 {
                match fence {
                    Some((f, len)) if f == c && n >= len => fence = None,
                    None => fence = Some((c, n)),
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_some() || line.starts_with("    ") || line.starts_with('\t') {
            continue;
        }
        inline_images(line, &mut out);
        html_images(line, &mut out);
        if let Some(src) = reference_definition(trimmed) {
            out.push(src);
        }
    }
    out
}

fn inline_images(line: &str, out: &mut Vec<String>) {
    let mut rest = line;
    while let Some(start) = rest.find("![") {
        rest = &rest[start + 2..];
        // the alt text, brackets nested
        let mut depth = 1;
        let mut end = None;
        for (i, c) in rest.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { return };
        rest = &rest[end + 1..];
        let Some(dest) = rest.strip_prefix('(') else {
            continue;
        };
        let dest = dest.trim_start();
        let src = if let Some(angled) = dest.strip_prefix('<') {
            angled.split('>').next().unwrap_or("")
        } else {
            let mut depth = 0;
            let end = dest
                .char_indices()
                .find(|(_, c)| match c {
                    '(' => {
                        depth += 1;
                        false
                    }
                    ')' if depth == 0 => true,
                    ')' => {
                        depth -= 1;
                        false
                    }
                    c => c.is_whitespace(),
                })
                .map_or(dest.len(), |(i, _)| i);
            &dest[..end]
        };
        if !src.is_empty() {
            out.push(src.to_string());
        }
    }
}

fn html_images(line: &str, out: &mut Vec<String>) {
    let lower = line.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img") {
        let tag_start = from + at;
        let tag_end = lower[tag_start..]
            .find('>')
            .map_or(lower.len(), |e| tag_start + e);
        let tag = &line[tag_start..tag_end];
        let tag_lower = &lower[tag_start..tag_end];
        if let Some(s) = tag_lower.find("src=") {
            let value = &tag[s + 4..];
            let src = match value.chars().next() {
                Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
                _ => value
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or(""),
            };
            if !src.is_empty() {
                out.push(src.to_string());
            }
        }
        from = tag_end;
    }
}

/// `[id]: src "title"` (up to three spaces of indent).
fn reference_definition(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix('[')?;
    let (label, rest) = rest.split_once("]:")?;
    if label.is_empty() || label.starts_with('^') {
        return None;
    }
    let dest = rest.trim_start();
    let src = match dest.strip_prefix('<') {
        Some(angled) => angled.split('>').next()?,
        None => dest.split_whitespace().next()?,
    };
    image_type(src).map(|_| src.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_picture_sources_outside_code() {
        let text = "# Doc\n![logo](img/logo.png \"Logo\") and ![a [b]](<pics/a b.png>)\n\
                    ```md\n![skip](no.png)\n```\n    ![indented](no2.png)\n\
                    <p><img width=10 src=\"html.svg\"/></p>\n[ref]: ./ref.jpg\n\
                    [link]: https://example.com\n![remote](https://x/y.png)\n";
        assert_eq!(
            image_sources(text),
            [
                "img/logo.png",
                "pics/a b.png",
                "html.svg",
                "./ref.jpg",
                "https://x/y.png"
            ]
        );
    }

    #[test]
    fn reads_pictures_at_the_side_revision() {
        set_read_markdown_files(true);
        let source = Diff::Text {
            hunks: Vec::new(),
            warnings: Default::default(),
        };
        let diff = markdown_diff(
            "docs/README.md",
            FileStatusKind::Modified,
            || Some(b"![a](img/a.png) ![b](../b.png) ![c](https://x/c.png)".to_vec()),
            || Some(b"old".to_vec()),
            source,
            |side, rel| (side == Side::Current && rel == "docs/img/a.png").then(|| b"PNG".to_vec()),
        )
        .unwrap();
        let Diff::Markdown {
            previous, current, ..
        } = diff
        else {
            panic!("not a Markdown diff")
        };
        let current = current.unwrap();
        assert_eq!(current.images.len(), 1);
        assert_eq!(current.images[0].0, "img/a.png");
        assert_eq!(previous.unwrap().text, "old");
        assert!(corvene_models::is_markdown_file("a/README.MD"));
        assert!(corvene_models::is_markdown_file("post.mdx"));
        assert!(!corvene_models::is_markdown_file("notes.txt"));
    }
}
