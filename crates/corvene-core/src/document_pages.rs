//! Corvene `1325-document-pages`: the Pages view of a document diff. Each
//! version of a Word, PowerPoint, OpenDocument, EPUB or RTF file is turned
//! into a PDF by LibreOffice (`soffice --headless --convert-to pdf`, with a
//! profile of its own so a running LibreOffice is left alone), and the two
//! PDFs are shown by the `1322-pdf-diff` page view.
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

static CONVERTED: LazyLock<Mutex<Converted>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The Pages diff of a document diff: a `Diff::Image` of the two versions
/// as PDF files (`corvene_git::PDF_MEDIA_TYPE`). Blocks for a few seconds
/// per version; call it off the UI thread.
pub fn pages_diff(
    path: &str,
    kind: FileStatusKind,
    previous: Option<&DocBlob>,
    current: Option<&DocBlob>,
) -> Result<Diff, String> {
    let program = soffice().ok_or("LibreOffice is not installed.")?;
    let ext = path.rsplit_once('.').map_or("docx", |(_, e)| e);
    let side = |blob: Option<&DocBlob>| -> Result<Option<ImageBlob>, String> {
        let Some(blob) = blob.filter(|b| !b.file.is_empty()) else {
            return Ok(None);
        };
        let pdf = to_pdf(program, &blob.file, ext)?;
        Ok(Some(ImageBlob {
            bytes: pdf.as_ref().clone(),
            media_type: corvene_git::PDF_MEDIA_TYPE.to_string(),
            original: None,
        }))
    };
    let current = if kind == FileStatusKind::Deleted {
        None
    } else {
        side(current)?
    };
    let previous = if kind.is_new_or_untracked() {
        None
    } else {
        side(previous)?
    };
    if previous.is_none() && current.is_none() {
        return Err("Neither version could be read.".into());
    }
    Ok(Diff::Image { previous, current })
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
    if let Ok(mut cache) = CONVERTED.lock() {
        if cache.len() >= CACHE
            && let Some(oldest) = cache.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k)
        {
            cache.remove(&oldest);
        }
        cache.insert(key, (Instant::now(), pdf.clone()));
    }
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
    fn profile_urls() {
        assert_eq!(file_url(Path::new("/tmp/a b")), "file:///tmp/a%20b");
        assert_eq!(file_url(Path::new(r"C:\x\y")), "file:///C:/x/y");
    }
}
