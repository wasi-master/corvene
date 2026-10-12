//! Corvene `1325-document-pages`: the Pages view of a document diff. Each
//! version of a Word, PowerPoint, OpenDocument, EPUB or RTF file is turned
//! into a PDF by LibreOffice (`soffice --headless --convert-to pdf`, with a
//! profile of its own so a running LibreOffice is left alone), and the two
//! PDFs are shown by the `1322-pdf-diff` page view.
//!
//! Corvene `1358-document-own-pages`: without LibreOffice, or when it cannot
//! convert a version, Corvene lays out its own reading of the document
//! (the blocks the Rendered view shows) on pages of the file's size and
//! writes the PDF itself (`corvene_paginate`). Those pages are not the
//! pages Word or LibreOffice would print: the file's fonts, sizes,
//! spacing, headers and footers are not read.
//!
//! GitHub Desktop has no document diff (`app/src/lib/git/diff.ts`
//! `getDiffType` makes these files binary).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use corvene_models::{Diff, DocBlob, FileStatusKind, ImageBlob};

/// How long one conversion may take.
const TIMEOUT: Duration = Duration::from_secs(90);
/// Converted files kept (by the hash of their bytes).
const CACHE: usize = 16;

/// The LibreOffice program, looked for once: `CORVENE_SOFFICE`, `soffice`
/// or `libreoffice` on `PATH`, then the usual install folders.
pub fn soffice() -> Option<&'static Path> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND.get_or_init(find_soffice).as_deref()
}

fn find_soffice() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CORVENE_SOFFICE") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let names: &[&str] = if cfg!(windows) {
        &["soffice.exe", "soffice.com"]
    } else {
        &["soffice", "libreoffice"]
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let known: &[&str] = if cfg!(target_os = "macos") {
        &["/Applications/LibreOffice.app/Contents/MacOS/soffice"]
    } else if cfg!(windows) {
        &[
            r"C:\Program Files\LibreOffice\program\soffice.exe",
            r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
        ]
    } else {
        &[
            "/usr/bin/soffice",
            "/usr/lib/libreoffice/program/soffice",
            "/opt/libreoffice/program/soffice",
            "/snap/bin/libreoffice",
        ]
    };
    known.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// Converted PDFs by the hash of their source, with when they were last
/// used.
type Converted = HashMap<u64, (Instant, Arc<Vec<u8>>)>;

static CONVERTED: LazyLock<Mutex<Converted>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The Pages view can be drawn: LibreOffice is installed, or Corvene may
/// lay out the pages itself (`own`, `1358-document-own-pages`).
pub fn available(own: bool) -> bool {
    own || soffice().is_some()
}

/// The Pages diff of a document diff: a `Diff::Image` of the two versions
/// as PDF files (`corvene_git::PDF_MEDIA_TYPE`), drawn by LibreOffice, or
/// by Corvene (`own`) when LibreOffice is missing or fails. `progress`
/// gets the fraction done (0 to 1) now and then. Blocks for a few seconds
/// per version; call it off the UI thread.
pub fn pages_diff(
    path: &str,
    kind: FileStatusKind,
    previous: Option<&DocBlob>,
    current: Option<&DocBlob>,
    own: bool,
    progress: &dyn Fn(f32),
) -> Result<Diff, String> {
    let program = soffice();
    if program.is_none() && !own {
        return Err("LibreOffice is not installed.".into());
    }
    let ext = path.rsplit_once('.').map_or("docx", |(_, e)| e);
    let current = current.filter(|_| kind != FileStatusKind::Deleted);
    let previous = previous.filter(|_| !kind.is_new_or_untracked());
    let sides = usize::from(current.is_some()) + usize::from(previous.is_some());
    let mut done = 0usize;
    let mut side = |blob: Option<&DocBlob>| -> Result<Option<ImageBlob>, String> {
        let Some(blob) = blob else {
            return Ok(None);
        };
        let base = done as f32 / sides.max(1) as f32;
        let part = |f: f32| progress(base + f / sides.max(1) as f32);
        let converted = match program {
            Some(program) if !blob.file.is_empty() => to_pdf(program, &blob.file, ext),
            _ => Err("LibreOffice is not installed.".into()),
        };
        let pdf = match converted {
            Ok(pdf) => pdf,
            Err(e) if !own => return Err(e),
            Err(e) => {
                if program.is_some() {
                    tracing::debug!(path, error = %e, "LibreOffice failed; laying out pages");
                }
                own_pdf(blob, &part)?
            }
        };
        done += 1;
        progress(done as f32 / sides.max(1) as f32);
        Ok(Some(ImageBlob {
            bytes: pdf.as_ref().clone(),
            media_type: corvene_git::PDF_MEDIA_TYPE.to_string(),
            original: None,
        }))
    };
    let previous = side(previous)?;
    let current = side(current)?;
    if previous.is_none() && current.is_none() {
        return Err("Neither version could be read.".into());
    }
    Ok(Diff::Image { previous, current })
}

/// The page size documents without one get: US Letter in the countries
/// that use it, else A4.
fn default_geometry() -> corvene_paginate::Geometry {
    static COUNTRY: OnceLock<Option<String>> = OnceLock::new();
    corvene_paginate::default_geometry(
        COUNTRY
            .get_or_init(corvene_platform::locale::country_code)
            .as_deref(),
    )
}

/// `1358-document-own-pages`: the document laid out by Corvene as a PDF.
fn own_pdf(blob: &DocBlob, progress: &dyn Fn(f32)) -> Result<Arc<Vec<u8>>, String> {
    let mut h = DefaultHasher::new();
    "own-pages".hash(&mut h);
    if blob.file.is_empty() {
        format!("{:?}", blob.sections).hash(&mut h);
    } else {
        blob.file.hash(&mut h);
    }
    let key = h.finish();
    if let Ok(mut cache) = CONVERTED.lock()
        && let Some((used, pdf)) = cache.get_mut(&key)
    {
        *used = Instant::now();
        return Ok(pdf.clone());
    }
    let pdf = corvene_paginate::document_pdf(blob, default_geometry(), &mut |f| progress(f))?;
    let pdf = Arc::new(pdf);
    remember(key, &pdf);
    Ok(pdf)
}

fn remember(key: u64, pdf: &Arc<Vec<u8>>) {
    if let Ok(mut cache) = CONVERTED.lock() {
        if cache.len() >= CACHE
            && let Some(oldest) = cache.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k)
        {
            cache.remove(&oldest);
        }
        cache.insert(key, (Instant::now(), pdf.clone()));
    }
}

/// `bytes` (a file with extension `ext`) as a PDF file.
fn to_pdf(program: &Path, bytes: &[u8], ext: &str) -> Result<Arc<Vec<u8>>, String> {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    ext.hash(&mut h);
    let key = h.finish();
    if let Ok(mut cache) = CONVERTED.lock()
        && let Some((used, pdf)) = cache.get_mut(&key)
    {
        *used = Instant::now();
        return Ok(pdf.clone());
    }
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let input = dir.path().join(format!("document.{ext}"));
    std::fs::write(&input, bytes).map_err(|e| e.to_string())?;
    let out = dir.path().join("out");
    let profile = dir.path().join("profile");
    let mut child = Command::new(program)
        .arg(format!("-env:UserInstallation={}", file_url(&profile)))
        .args([
            "--headless",
            "--norestore",
            "--nologo",
            "--nodefault",
            "--convert-to",
            "pdf",
            "--outdir",
        ])
        .arg(&out)
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("LibreOffice did not start: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("LibreOffice took too long.".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let pdf = std::fs::read(out.join("document.pdf"))
        .map_err(|_| "LibreOffice could not convert the file.".to_string())?;
    let pdf = Arc::new(pdf);
    remember(key, &pdf);
    Ok(pdf)
}

/// A `file://` URL of a local folder (LibreOffice's profile option).
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text: String = text
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            c => c.to_string(),
        })
        .collect();
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pages_without_a_file() {
        use corvene_models::{DocBlock, DocKind, DocRun, DocSection};
        let blob = |text: &str| DocBlob {
            kind: DocKind::Text,
            sections: vec![DocSection {
                blocks: vec![DocBlock::Heading {
                    level: 1,
                    runs: vec![DocRun::plain(text)],
                }],
                ..DocSection::default()
            }],
            images: Vec::new(),
            truncated: false,
            page: None,
            file: Default::default(),
        };
        let (old, new) = (blob("Old title"), blob("New title"));
        let diff = pages_diff(
            "a.docx",
            FileStatusKind::Modified,
            Some(&old),
            Some(&new),
            true,
            &|_| {},
        )
        .expect("pages");
        let Diff::Image {
            previous: Some(previous),
            current: Some(current),
        } = diff
        else {
            panic!("two PDFs");
        };
        let pdf = crate::pdf::PdfDocument::open(current.bytes).expect("a PDF");
        assert_eq!(pdf.page_count(), 1);
        if pdf.text().is_some_and(|t| !t.trim().is_empty()) {
            assert!(pdf.text().unwrap_or_default().contains("New title"));
        }
        assert!(pdf.render(0, 800).is_some(), "the page draws");
        assert!(crate::pdf::PdfDocument::open(previous.bytes).is_ok());
    }

    #[test]
    fn profile_urls() {
        assert_eq!(file_url(Path::new("/tmp/a b")), "file:///tmp/a%20b");
        assert_eq!(file_url(Path::new(r"C:\x\y")), "file:///C:/x/y");
    }
}
