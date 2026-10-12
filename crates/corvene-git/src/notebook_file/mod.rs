//! Corvene `1331-notebook-diff`: Jupyter notebooks (`.ipynb`) read into
//! cells (source, outputs, metadata, attachments), so their diff can show
//! the notebook cell by cell next to git's own JSON diff.
//!
//! GitHub Desktop shows a notebook as the text diff of its JSON
//! (`app/src/lib/git/diff.ts`), where base64 images and output churn bury
//! the code (desktop/desktop#5279, #12121, #17728). nbformat 3 notebooks
//! are upgraded the way `nbformat.v4.convert` does (heading cells become
//! markdown, `pyout` / `pyerr` outputs `execute_result` / `error`). Each
//! cell keeps its line range in the file, so staging a cell selects its
//! lines in git's diff.

mod json_spans;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{
    Diff, FileStatusKind, ImageBlob, NbAttachment, NbCell, NbCellKind, NbData, NbMime, NbOutput,
    NbOutputKind, NotebookBlob,
};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

pub use crate::linked_files::Side;
use crate::linked_files::{Budget, image_type, percent_decode, resolve};

/// Set by the dispatcher from the flag.
static READ_NOTEBOOKS: AtomicBool = AtomicBool::new(false);

/// Turn notebook diffs on or off.
pub fn set_read_notebooks(enabled: bool) {
    READ_NOTEBOOKS.store(enabled, Ordering::Relaxed);
}

fn read_notebooks() -> bool {
    READ_NOTEBOOKS.load(Ordering::Relaxed)
}

/// A Jupyter notebook while the flag is on.
pub fn is_notebook(path: &str) -> bool {
    read_notebooks() && corvene_models::is_notebook_file(path)
}

/// Larger files keep git's diff only.
pub const MAX_BYTES: usize = 64 << 20;
/// Images markdown cells link to, read per side.
const MAX_LINKED_IMAGES: usize = 16;
/// Bytes of linked images read per side.
const MAX_LINKED_BYTES: usize = 8 << 20;

/// A change of a notebook as a [`Diff::Notebook`] next to git's diff
/// `source`; `None` when the flag is off, the path is not a notebook, a
/// side is too large or does not read (the caller keeps `source`).
/// `linked` reads a repository file (by repository-relative path) at a
/// side's revision, for images markdown cells link to.
pub fn notebook_diff(
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
    source: Diff,
    linked: impl Fn(Side, &str) -> Option<Vec<u8>>,
) -> Option<Diff> {
    if !is_notebook(path) {
        return None;
    }
    // `796-lfs-text-diff` turns a pointer diff into the contents' diff
    if source.hunks().is_some_and(crate::lfs::is_pointer_diff) {
        return None;
    }
    let side = |which: Side, bytes: Option<Vec<u8>>| -> Option<Option<Arc<NotebookBlob>>> {
        match bytes {
            None => Some(None),
            Some(bytes) if bytes.len() > MAX_BYTES => None,
            Some(bytes) => match read(&bytes) {
                Ok(mut blob) => {
                    blob.linked_images = linked_images(path, &blob, |rel| linked(which, rel));
                    Some(Some(Arc::new(blob)))
                }
                Err(e) => {
                    tracing::debug!(path, error = %e, "notebook did not read");
                    None
                }
            },
        }
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
    Some(Diff::Notebook {
        previous,
        current,
        source: Box::new(source),
    })
}

/// One version of a notebook.
pub fn read(bytes: &[u8]) -> Result<NotebookBlob, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let doc: J = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if !matches!(doc, J::Obj(_)) {
        return Err("not a JSON object".into());
    }
    let spans = json_spans::cell_spans(text)?;
    let major = doc
        .get("nbformat")
        .and_then(J::as_u64)
        .or_else(|| doc.get("worksheets").map(|_| 3))
        .unwrap_or(4);
    let minor = doc.get("nbformat_minor").and_then(J::as_u64).unwrap_or(0);
    let v3 = major < 4;
    let (cells, spans) = if v3 {
        let cells = doc
            .get("worksheets")
            .and_then(J::as_arr)
            .and_then(|w| w.first())
            .and_then(|w| w.get("cells"));
        (cells, spans.worksheet_cells)
    } else {
        (doc.get("cells"), spans.cells)
    };
    let cells: &[J] = match cells {
        Some(J::Arr(cells)) => cells,
        Some(_) => return Err("cells is not an array".into()),
        None if v3 => &[],
        None => return Err("no cells".into()),
    };
    let spans = spans.unwrap_or_default();
    if spans.len() != cells.len() {
        return Err(format!(
            "{} cells but {} cell spans",
            cells.len(),
            spans.len()
        ));
    }
    let lines = json_spans::line_ranges(text, &spans);
    let cells = cells
        .iter()
        .zip(lines)
        .map(|(cell, lines)| {
            let mut cell = if v3 { cell_v3(cell) } else { cell_v4(cell) };
            cell.lines = lines;
            cell
        })
        .collect();

    let metadata = doc.get("metadata");
    let lang_info = metadata.and_then(|m| m.get("language_info"));
    let kernelspec = metadata.and_then(|m| m.get("kernelspec"));
    let language = lang_info
        .and_then(|l| l.get("name"))
        .and_then(J::as_str)
        .or_else(|| {
            kernelspec
                .and_then(|k| k.get("language"))
                .and_then(J::as_str)
        })
        .or_else(|| metadata.and_then(|m| m.get("language")).and_then(J::as_str))
        .filter(|l| !l.is_empty())
        .unwrap_or("python")
        .to_string();
    let file_extension = lang_info
        .and_then(|l| l.get("file_extension"))
        .and_then(J::as_str)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| language_extension(&language).to_string());
    let kernel = kernelspec
        .and_then(|k| k.get("display_name"))
        .and_then(J::as_str)
        .map(str::to_string);
    Ok(NotebookBlob {
        format: (
            u32::try_from(major).unwrap_or(u32::MAX),
            u32::try_from(minor).unwrap_or(u32::MAX),
        ),
        language,
        file_extension,
        kernel,
        metadata: pairs(metadata),
        cells,
        linked_images: Vec::new(),
        raw: Arc::from(text),
    })
}

/// The file extension of a kernel language, for highlighting.
fn language_extension(language: &str) -> &'static str {
    match language.to_ascii_lowercase().as_str() {
        "python" | "python3" | "ipython" => ".py",
        "julia" => ".jl",
        "r" => ".r",
        "scala" => ".scala",
        "javascript" => ".js",
        "typescript" => ".ts",
        "ruby" => ".rb",
        "c++" | "cpp" => ".cpp",
        "rust" => ".rs",
        "go" => ".go",
        "bash" | "sh" => ".sh",
        "sql" => ".sql",
        "matlab" | "octave" => ".m",
        "haskell" => ".hs",
        "java" => ".java",
        "kotlin" => ".kt",
        "csharp" | "c#" => ".cs",
        "fsharp" | "f#" => ".fs",
        "powershell" => ".ps1",
        "perl" => ".pl",
        "lua" => ".lua",
        _ => ".txt",
    }
}

fn cell_kind(name: Option<&str>) -> NbCellKind {
    match name {
        Some("code") => NbCellKind::Code,
        Some("markdown") => NbCellKind::Markdown,
        _ => NbCellKind::Raw,
    }
}

/// An nbformat 4 cell.
fn cell_v4(cell: &J) -> NbCell {
    let kind = cell_kind(cell.get("cell_type").and_then(J::as_str));
    let outputs = outputs(cell.get("outputs"), false);
    NbCell {
        id: cell.get("id").and_then(J::as_str).map(str::to_string),
        kind,
        source: text(cell.get("source")),
        execution_count: cell.get("execution_count").and_then(J::as_u64),
        outputs,
        metadata: pairs(cell.get("metadata")),
        attachments: attachments(cell.get("attachments")),
        lines: (0, 0),
    }
}

/// An nbformat 3 cell, upgraded.
fn cell_v3(cell: &J) -> NbCell {
    let cell_type = cell.get("cell_type").and_then(J::as_str);
    let (kind, source, execution_count, outputs) = match cell_type {
        Some("code") => (
            NbCellKind::Code,
            text(cell.get("input")),
            cell.get("prompt_number").and_then(J::as_u64),
            self::outputs(cell.get("outputs"), true),
        ),
        Some("heading") => {
            let level = cell
                .get("level")
                .and_then(J::as_u64)
                .unwrap_or(1)
                .clamp(1, 6);
            let source = text(cell.get("source"));
            let hashes = "#".repeat(usize::try_from(level).unwrap_or(1));
            (
                NbCellKind::Markdown,
                format!("{hashes} {source}"),
                None,
                Vec::new(),
            )
        }
        other => (cell_kind(other), text(cell.get("source")), None, Vec::new()),
    };
    NbCell {
        id: None,
        kind,
        source,
        execution_count,
        outputs,
        metadata: pairs(cell.get("metadata")),
        attachments: Vec::new(),
        lines: (0, 0),
    }
}

/// A code cell's outputs, consecutive streams of the same name merged.
fn outputs(list: Option<&J>, v3: bool) -> Vec<NbOutput> {
    let mut out: Vec<NbOutput> = Vec::new();
    for output in list.and_then(J::as_arr).unwrap_or_default() {
        let Some(output) = (if v3 {
            output_v3(output)
        } else {
            output_v4(output)
        }) else {
            continue;
        };
        if output.kind == NbOutputKind::Stream
            && let Some(last) = out.last_mut()
            && last.kind == NbOutputKind::Stream
            && last.name == output.name
        {
            last.text.push_str(&output.text);
            continue;
        }
        out.push(output);
    }
    out
}

fn empty_output(kind: NbOutputKind) -> NbOutput {
    NbOutput {
        kind,
        name: String::new(),
        text: String::new(),
        evalue: String::new(),
        execution_count: None,
        data: Vec::new(),
        metadata: Vec::new(),
    }
}

fn error_output(output: &J) -> NbOutput {
    let traceback = output
        .get("traceback")
        .and_then(J::as_arr)
        .unwrap_or_default()
        .iter()
        .filter_map(J::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    NbOutput {
        name: str_of(output.get("ename")),
        evalue: str_of(output.get("evalue")),
        text: traceback,
        ..empty_output(NbOutputKind::Error)
    }
}

fn output_v4(output: &J) -> Option<NbOutput> {
    let kind = match output.get("output_type").and_then(J::as_str)? {
        "stream" => {
            return Some(NbOutput {
                name: str_of(output.get("name")),
                text: text(output.get("text")),
                ..empty_output(NbOutputKind::Stream)
            });
        }
        "error" => return Some(error_output(output)),
        "display_data" | "update_display_data" => NbOutputKind::DisplayData,
        "execute_result" => NbOutputKind::ExecuteResult,
        _ => return None,
    };
    let data = match output.get("data") {
        Some(J::Obj(members)) => bundle(members.iter().map(|(k, v)| (k.as_str(), v))),
        _ => Vec::new(),
    };
    Some(NbOutput {
        execution_count: output.get("execution_count").and_then(J::as_u64),
        data,
        metadata: pairs(output.get("metadata")),
        ..empty_output(kind)
    })
}

fn output_v3(output: &J) -> Option<NbOutput> {
    let kind = match output.get("output_type").and_then(J::as_str)? {
        "stream" => {
            let name = output
                .get("stream")
                .and_then(J::as_str)
                .unwrap_or("stdout")
                .to_string();
            return Some(NbOutput {
                name,
                text: text(output.get("text")),
                ..empty_output(NbOutputKind::Stream)
            });
        }
        "pyerr" | "error" => return Some(error_output(output)),
        "display_data" => NbOutputKind::DisplayData,
        "pyout" | "execute_result" => NbOutputKind::ExecuteResult,
        _ => return None,
    };
    // v3 keeps the MIME bundle in the output itself, under short keys
    let data = match output {
        J::Obj(members) => bundle(
            members
                .iter()
                .filter_map(|(k, v)| v3_mime(k).map(|m| (m, v))),
        ),
        _ => Vec::new(),
    };
    Some(NbOutput {
        execution_count: output.get("prompt_number").and_then(J::as_u64),
        data,
        metadata: pairs(output.get("metadata")),
        ..empty_output(kind)
    })
}

/// The MIME type of an nbformat 3 output key (`png` → `image/png`).
fn v3_mime(key: &str) -> Option<&str> {
    Some(match key {
        "text" => "text/plain",
        "html" => "text/html",
        "png" => "image/png",
        "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "latex" => "text/latex",
        "json" => "application/json",
        "javascript" => "application/javascript",
        "markdown" => "text/markdown",
        "pdf" => "application/pdf",
        key if key.contains('/') => key,
        _ => return None,
    })
}

/// `attachments`: `{name: {mime: data}}`, sorted by name.
fn attachments(value: Option<&J>) -> Vec<NbAttachment> {
    let Some(J::Obj(members)) = value else {
        return Vec::new();
    };
    let mut out: Vec<NbAttachment> = Vec::new();
    for (name, bundle_value) in members {
        let data = match bundle_value {
            J::Obj(m) => bundle(m.iter().map(|(k, v)| (k.as_str(), v))),
            _ => Vec::new(),
        };
        out.retain(|a| a.name != *name);
        out.push(NbAttachment {
            name: name.clone(),
            data,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A MIME bundle in file order (a type given twice keeps its last value).
fn bundle<'a>(members: impl Iterator<Item = (&'a str, &'a J)>) -> Vec<NbMime> {
    let mut out: Vec<NbMime> = Vec::new();
    for (mime, value) in members {
        let data = mime_data(mime, value);
        if let Some(existing) = out.iter_mut().find(|m| m.mime == mime) {
            existing.data = data;
        } else {
            out.push(NbMime {
                mime: mime.to_string(),
                data,
            });
        }
    }
    out
}

/// Binary payloads notebooks store as base64.
fn is_binary_mime(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    (mime.starts_with("image/") && !mime.starts_with("image/svg"))
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
        || matches!(
            mime.as_str(),
            "application/pdf" | "application/octet-stream" | "application/zip"
        )
}

fn is_json_mime(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime == "application/json" || mime.ends_with("+json")
}

fn mime_data(mime: &str, value: &J) -> NbData {
    let lines = matches!(value, J::Arr(items) if items.iter().all(|i| matches!(i, J::Str(_))));
    if is_json_mime(mime) {
        // nbformat 4 stores the value itself; some writers a string of it
        return match value {
            J::Str(s) => match serde_json::from_str::<J>(s) {
                Ok(parsed) => NbData::Json(sorted_pretty(&parsed)),
                Err(_) => NbData::Text(s.clone()),
            },
            value => NbData::Json(sorted_pretty(value)),
        };
    }
    match value {
        J::Str(_) => {}
        J::Arr(_) if lines => {}
        value => return NbData::Json(sorted_pretty(value)),
    }
    let text = text(Some(value));
    if is_binary_mime(mime) {
        NbData::Base64(text.chars().filter(|c| !c.is_whitespace()).collect())
    } else {
        NbData::Text(text)
    }
}

/// An object's keys with their pretty-printed values, sorted by key.
fn pairs(value: Option<&J>) -> Vec<(String, String)> {
    let Some(J::Obj(members)) = value else {
        return Vec::new();
    };
    let mut map = std::collections::BTreeMap::new();
    for (k, v) in members {
        map.insert(k.as_str(), v);
    }
    map.into_iter()
        .map(|(k, v)| (k.to_string(), sorted_pretty(v)))
        .collect()
}

/// A string, or an array of strings joined (each carries its newline).
fn text(value: Option<&J>) -> String {
    match value {
        Some(J::Str(s)) => s.clone(),
        Some(J::Arr(items)) => items.iter().filter_map(J::as_str).collect(),
        _ => String::new(),
    }
}

fn str_of(value: Option<&J>) -> String {
    value.and_then(J::as_str).unwrap_or_default().to_string()
}

/// `value` as pretty-printed JSON with object keys sorted at every level.
pub(crate) fn sorted_pretty(value: &J) -> String {
    serde_json::to_string_pretty(&sorted_value(value)).unwrap_or_default()
}

fn sorted_value(value: &J) -> serde_json::Value {
    use serde_json::Value;
    match value {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::Num(n) => Value::Number(n.clone()),
        J::Str(s) => Value::String(s.clone()),
        J::Arr(items) => Value::Array(items.iter().map(sorted_value).collect()),
        J::Obj(members) => {
            let mut sorted: std::collections::BTreeMap<&str, &J> = Default::default();
            for (k, v) in members {
                sorted.insert(k, v);
            }
            // inserted in key order, so the map is sorted with or without
            // serde_json's `preserve_order`
            let mut map = serde_json::Map::new();
            for (k, v) in sorted {
                map.insert(k.to_string(), sorted_value(v));
            }
            Value::Object(map)
        }
    }
}

// ---------------------------------------------------------------------------
// Linked images
// ---------------------------------------------------------------------------

/// Images markdown cells link to by relative path, read with `read`.
fn linked_images(
    path: &str,
    blob: &NotebookBlob,
    read: impl Fn(&str) -> Option<Vec<u8>>,
) -> Vec<(String, ImageBlob)> {
    let mut out: Vec<(String, ImageBlob)> = Vec::new();
    let mut budget = Budget {
        files: MAX_LINKED_IMAGES,
        bytes: MAX_LINKED_BYTES,
    };
    for cell in blob.cells.iter().filter(|c| c.kind == NbCellKind::Markdown) {
        for link in image_links(&cell.source) {
            if budget.files == 0 {
                return out;
            }
            if out.iter().any(|(l, _)| *l == link) {
                continue;
            }
            let target = link
                .split(['#', '?'])
                .next()
                .map(percent_decode)
                .unwrap_or_default();
            let Some(rel) = resolve(path, &target) else {
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
                link,
                ImageBlob {
                    bytes,
                    media_type: media_type.to_string(),
                    original: None,
                },
            ));
        }
    }
    out
}

/// The targets of `![alt](target)` and `<img src="target">` in markdown,
/// as written.
pub(crate) fn image_links(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    // `![alt](target "title")`
    let mut rest = markdown;
    while let Some(at) = rest.find("![") {
        rest = &rest[at + 2..];
        let Some(close) = rest.find("](") else {
            break;
        };
        // the alt text stays on one line
        if rest[..close].contains('\n') {
            continue;
        }
        let after = &rest[close + 2..];
        let after = after.trim_start_matches([' ', '\t']);
        let target = if let Some(angled) = after.strip_prefix('<') {
            angled.split('>').next().unwrap_or_default()
        } else {
            after
                .split(|c: char| c.is_whitespace() || c == ')')
                .next()
                .unwrap_or_default()
        };
        if !target.is_empty() {
            out.push(target.to_string());
        }
        rest = after;
    }
    // `<img src="target">`
    let lower = markdown.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img") {
        let start = from + at + 4;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        if !lower[start..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_whitespace() || b == b'/' || b == b'>')
        {
            from = start;
            continue;
        }
        let tag = &lower[start..end];
        if let Some(src) = find_attr(tag, "src") {
            let (a, b) = src;
            let value = &markdown[start + a..start + b];
            if !value.is_empty() {
                out.push(value.to_string());
            }
        }
        from = end;
    }
    out
}

/// The byte range of an attribute's value inside a (lower-cased) tag.
fn find_attr(tag: &str, name: &str) -> Option<(usize, usize)> {
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(at) = tag[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        // a whole attribute name: preceded by whitespace
        if start == 0 || !bytes[start - 1].is_ascii_whitespace() {
            continue;
        }
        let mut i = from;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            continue;
        }
        i += 1;
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        return match bytes.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let s = i + 1;
                let e = tag[s..].find(q as char).map_or(tag.len(), |e| s + e);
                Some((s, e))
            }
            Some(_) => {
                let e = tag[i..]
                    .find(|c: char| c.is_ascii_whitespace())
                    .map_or(tag.len(), |e| i + e);
                Some((i, e))
            }
            None => None,
        };
    }
    None
}

// ---------------------------------------------------------------------------
// JSON values in file order
// ---------------------------------------------------------------------------

/// A JSON value that keeps object members in file order (MIME bundles
/// list their types in the order Jupyter prefers), whatever
/// `serde_json`'s `preserve_order` feature is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum J {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    /// A member of an object (the last one when the key repeats).
    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(members) => members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            J::Num(n) => n.as_u64().or_else(|| {
                n.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0 && *f < u64::MAX as f64)
                    .map(|f| f as u64)
            }),
            _ => None,
        }
    }

    fn as_arr(&self) -> Option<&[J]> {
        match self {
            J::Arr(items) => Some(items),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JVisitor)
    }
}

struct JVisitor;

impl<'de> Visitor<'de> for JVisitor {
    type Value = J;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<J, E> {
        Ok(J::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<J, E> {
        Ok(J::Num(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<J, E> {
        Ok(J::Num(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<J, E> {
        Ok(serde_json::Number::from_f64(v).map_or(J::Null, J::Num))
    }

    fn visit_str<E>(self, v: &str) -> Result<J, E> {
        Ok(J::Str(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<J, E> {
        Ok(J::Str(v))
    }

    fn visit_unit<E>(self) -> Result<J, E> {
        Ok(J::Null)
    }

    fn visit_none<E>(self) -> Result<J, E> {
        Ok(J::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<J, D::Error> {
        J::deserialize(d)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<J, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(J::Arr(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<J, A::Error> {
        let mut members = Vec::with_capacity(map.size_hint().unwrap_or(0).min(4096));
        while let Some((k, v)) = map.next_entry::<String, J>()? {
            members.push((k, v));
        }
        Ok(J::Obj(members))
    }
}
