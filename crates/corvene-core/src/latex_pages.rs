//! Corvene `1332-latex-diff`: the Pages view of a LaTeX document. Each
//! version is compiled to PDF by an installed TeX engine (latexmk, else
//! Tectonic) without shell escape, in a temporary copy of the document's
//! folder, and the two PDFs go to the PDF page diff (`1322-pdf-diff`).
//!
//! The other files a document reads (`\input` parts, the bibliography,
//! pictures) are the working copy's; only the diffed file itself and the
//! pictures it names are each version's own. A file without
//! `\documentclass` is compiled through the root its `%!TEX root = …`
//! line names.
//!
//! GitHub Desktop shows LaTeX files as text only.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use corvene_models::{Diff, FileStatusKind, ImageBlob, LatexBlob};

/// A compile that takes longer is stopped.
const TIMEOUT: Duration = Duration::from_secs(90);
/// The copied folder's size limit.
const MAX_COPY: u64 = 200 << 20;
/// Compiled PDFs kept.
const CACHE: usize = 8;

/// The TeX engine found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Engine {
    pub program: PathBuf,
    pub kind: EngineKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    Latexmk,
    Tectonic,
}

/// The TeX engine on this machine (`CORVENE_TEX` overrides), found once.
pub fn engine() -> Option<&'static Engine> {
    static FOUND: OnceLock<Option<Engine>> = OnceLock::new();
    FOUND.get_or_init(find_engine).as_ref()
}

fn find_engine() -> Option<Engine> {
    let kind_of = |p: &Path| {
        let name = p.file_stem()?.to_string_lossy().to_ascii_lowercase();
        match name.as_str() {
            "latexmk" => Some(EngineKind::Latexmk),
            "tectonic" => Some(EngineKind::Tectonic),
            _ => None,
        }
    };
    if let Some(path) = std::env::var_os("CORVENE_TEX") {
        let path = PathBuf::from(path);
        let kind = kind_of(&path)?;
        return path.is_file().then_some(Engine {
            program: path,
            kind,
        });
    }
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        // MacTeX installs here; apps started from Finder lack it on PATH
        dirs.push("/Library/TeX/texbin".into());
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    for (name, kind) in [
        ("latexmk", EngineKind::Latexmk),
        ("tectonic", EngineKind::Tectonic),
    ] {
        for dir in &dirs {
            let candidate = dir.join(format!("{name}{exe}"));
            if candidate.is_file() {
                return Some(Engine {
                    program: candidate,
                    kind,
                });
            }
        }
    }
    None
}

type Compiled = HashMap<u64, (Instant, Result<Arc<Vec<u8>>, String>)>;

static COMPILED: Mutex<Option<Compiled>> = Mutex::new(None);

/// The magic `%!TEX root = main.tex` line of a part of a document.
pub fn magic_root(text: &str) -> Option<String> {
    text.lines().take(20).find_map(|l| {
        let l = l.trim_start_matches('%').trim();
        let rest = l
            .strip_prefix("!TEX root")
            .or_else(|| l.strip_prefix("!TeX root"))?;
        Some(rest.trim().trim_start_matches('=').trim().to_string())
    })
}

/// Joins a relative path to a folder, `..` taken off it.
fn join(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The page diff of both versions of the LaTeX file `path` (relative to
/// `workdir`).
pub fn pages_diff(
    workdir: &Path,
    path: &str,
    kind: FileStatusKind,
    diff: &Diff,
) -> Result<Diff, String> {
    let engine = engine().ok_or("No TeX engine (latexmk or Tectonic) is installed.")?;
    let Diff::Latex {
        previous, current, ..
    } = diff
    else {
        return Err("This is not a LaTeX document.".into());
    };
    let side = |blob: Option<&LatexBlob>, which: &str| -> Result<Option<ImageBlob>, String> {
        let Some(blob) = blob else { return Ok(None) };
        let pdf = compile(engine, workdir, path, blob)
            .map_err(|e| format!("The {which} version did not compile: {e}"))?;
        Ok(Some(ImageBlob {
            bytes: pdf.as_ref().clone(),
            media_type: corvene_git::PDF_MEDIA_TYPE.to_string(),
            original: None,
        }))
    };
    let current = if kind == FileStatusKind::Deleted {
        None
    } else {
        side(current.as_deref(), "new")?
    };
    let previous = if kind.is_new_or_untracked() {
        None
    } else {
        side(previous.as_deref(), "old")?
    };
    if previous.is_none() && current.is_none() {
        return Err("Neither version could be compiled.".into());
    }
    Ok(Diff::Image { previous, current })
}

fn compile(
    engine: &Engine,
    workdir: &Path,
    path: &str,
    blob: &LatexBlob,
) -> Result<Arc<Vec<u8>>, String> {
    // what to compile: the file, or the root it names
    let root = if blob.text.contains("\\documentclass") {
        path.to_string()
    } else {
        let rel = magic_root(&blob.text).ok_or(
            "this file is part of a document (no \\documentclass); a `%!TEX root = main.tex` line \
             names the file to compile",
        )?;
        join(parent(path), &rel)
    };
    let base = parent(&root).to_string();
    let mut h = DefaultHasher::new();
    workdir.hash(&mut h);
    path.hash(&mut h);
    root.hash(&mut h);
    blob.text.hash(&mut h);
    for (p, i) in &blob.images {
        p.hash(&mut h);
        i.bytes.hash(&mut h);
    }
    let key = h.finish();
    if let Ok(mut cache) = COMPILED.lock()
        && let Some((used, pdf)) = cache.get_or_insert_with(HashMap::new).get_mut(&key)
    {
        *used = Instant::now();
        return pdf.clone();
    }
    let result = compile_uncached(engine, workdir, path, &root, &base, blob);
    if let Ok(mut cache) = COMPILED.lock() {
        let cache = cache.get_or_insert_with(HashMap::new);
        if cache.len() >= CACHE
            && let Some(oldest) = cache.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k)
        {
            cache.remove(&oldest);
        }
        cache.insert(key, (Instant::now(), result.clone()));
    }
    result
}

fn compile_uncached(
    engine: &Engine,
    workdir: &Path,
    path: &str,
    root: &str,
    base: &str,
    blob: &LatexBlob,
) -> Result<Arc<Vec<u8>>, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let copy = dir.path().join("src");
    let mut budget = MAX_COPY;
    copy_tree(&workdir.join(base), &copy, &mut budget, 0)?;
    // the version's own file and pictures over the working copy's
    let rel_in_copy = |repo_rel: &str| {
        let rel = repo_rel
            .strip_prefix(base)
            .map(|r| r.trim_start_matches('/'))
            .unwrap_or(repo_rel);
        copy.join(rel)
    };
    let file = rel_in_copy(path);
    if let Some(p) = file.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, &blob.text).map_err(|e| e.to_string())?;
    for (name, image) in &blob.images {
        let target = rel_in_copy(&join(parent(path), name));
        if let Some(p) = target.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(target, &image.bytes);
    }
    let main = rel_in_copy(root);
    let main_dir = main.parent().unwrap_or(&copy).to_path_buf();
    let main_name = main
        .file_name()
        .ok_or("no file to compile")?
        .to_string_lossy()
        .to_string();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut command = Command::new(&engine.program);
    command.current_dir(&main_dir);
    match engine.kind {
        EngineKind::Latexmk => {
            command
                .args([
                    "-pdf",
                    "-interaction=nonstopmode",
                    "-halt-on-error",
                    "-file-line-error",
                    "-no-shell-escape",
                ])
                .arg(format!("-outdir={}", out.display()));
        }
        EngineKind::Tectonic => {
            command
                .args(["--untrusted", "--keep-logs", "--outdir"])
                .arg(&out);
        }
    }
    command
        .arg(&main_name)
        .env("openout_any", "p")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| format!("the TeX engine did not start: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let stem = main_name.trim_end_matches(".tex").to_string();
    match std::fs::read(out.join(format!("{stem}.pdf"))) {
        Ok(pdf) => Ok(Arc::new(pdf)),
        Err(_) => {
            let log = std::fs::read_to_string(out.join(format!("{stem}.log"))).unwrap_or_default();
            Err(first_error(&log).unwrap_or_else(|| "no PDF was written".into()))
        }
    }
}

/// The first `! …` error of a TeX log (or a `file:line: error` line).
pub fn first_error(log: &str) -> Option<String> {
    log.lines()
        .find(|l| l.starts_with('!') || l.contains(".tex:") && l.contains(": "))
        .map(|l| l.trim_start_matches('!').trim().chars().take(200).collect())
}

/// Copies a folder (no hidden folders, build output or version control),
/// within `budget` bytes.
fn copy_tree(from: &Path, to: &Path, budget: &mut u64, depth: u32) -> Result<(), String> {
    if depth > 12 {
        return Ok(());
    }
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.')
            || matches!(
                name_str.as_ref(),
                "node_modules" | "target" | "_minted" | "__pycache__"
            )
        {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(&name), budget, depth + 1)?;
        } else if kind.is_file() {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if len > *budget {
                return Err("the document's folder is too large to copy".into());
            }
            *budget -= len;
            std::fs::copy(entry.path(), to.join(&name)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_and_paths() {
        assert_eq!(
            magic_root("% !TEX root = ../main.tex\n\\section{A}").as_deref(),
            Some("../main.tex")
        );
        assert_eq!(magic_root("\\section{A}"), None);
        assert_eq!(join("paper/chapters", "../main.tex"), "paper/main.tex");
        assert_eq!(join("", "./a/b.tex"), "a/b.tex");
        assert_eq!(
            first_error("This is pdfTeX\n! Undefined control sequence.\nl.3").as_deref(),
            Some("Undefined control sequence.")
        );
    }
}
