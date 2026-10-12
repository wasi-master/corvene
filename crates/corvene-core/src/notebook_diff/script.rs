//! The Script view: a notebook written out as a script in Jupytext's
//! "percent" format (`# %%` before each code cell, `# %% [markdown]` and
//! commented lines for markdown), with each cell's text outputs as
//! `# Out:` comments, so a plain line diff reads like a diff of code.

use corvene_models::{Diff, NbCellKind, NbOutputKind, NotebookBlob};

use super::outputs::{Render, renderings};

/// Output lines kept per output in the script.
const MAX_OUTPUT_LINES: usize = 40;

/// The comment prefix of a language (`#` unless it is a C-like or other
/// well-known one).
pub fn comment_prefix(language: &str, extension: &str) -> &'static str {
    let l = language.to_ascii_lowercase();
    match (l.as_str(), extension) {
        ("julia" | "python" | "r" | "ruby" | "bash" | "sh" | "perl" | "powershell", _) => "#",
        (
            _,
            ".js" | ".ts" | ".rs" | ".go" | ".java" | ".kt" | ".scala" | ".cs" | ".fs" | ".cpp",
        )
        | (
            "javascript" | "typescript" | "rust" | "go" | "java" | "kotlin" | "scala" | "c++"
            | "csharp" | "c#",
            _,
        ) => "//",
        ("sql", _) | (_, ".sql") | ("haskell", _) | (_, ".hs") | ("lua", _) => "--",
        ("matlab" | "octave", _) | (_, ".m") => "%",
        _ => "#",
    }
}

/// A notebook as a percent-format script.
pub fn script(nb: &NotebookBlob) -> String {
    let c = comment_prefix(&nb.language, &nb.file_extension);
    let mut out = String::new();
    for (i, cell) in nb.cells.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match cell.kind {
            NbCellKind::Code => {
                match cell.execution_count {
                    Some(n) => out.push_str(&format!("{c} %% In [{n}]\n")),
                    None => out.push_str(&format!("{c} %%\n")),
                }
                push_lines(&mut out, &cell.source, "");
                for output in &cell.outputs {
                    let label = match output.kind {
                        NbOutputKind::Stream => output.name.clone(),
                        NbOutputKind::Error => "error".to_string(),
                        _ => match output.execution_count {
                            Some(n) => format!("Out[{n}]"),
                            None => "Out".to_string(),
                        },
                    };
                    let rendering = renderings(output).into_iter().next();
                    let text = match rendering.as_ref().map(|r| &r.render) {
                        Some(Render::Ansi { text, .. }) => strip_ansi(text),
                        Some(Render::Table(rows)) => rows
                            .iter()
                            .map(|r| r.join(" | "))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        Some(Render::Image { mime, base64 }) => {
                            format!("<{mime}, {} bytes>", base64.len() * 3 / 4)
                        }
                        Some(Render::Pdf(b)) => {
                            format!("<application/pdf, {} bytes>", b.len() * 3 / 4)
                        }
                        Some(Render::Placeholder { mime, .. }) => format!("<{mime}>"),
                        Some(r) => r.text().unwrap_or_default().to_string(),
                        None => String::new(),
                    };
                    out.push_str(&format!("{c} {label}:\n"));
                    let mut lines: Vec<&str> = text.lines().collect();
                    let more = lines.len().saturating_sub(MAX_OUTPUT_LINES);
                    lines.truncate(MAX_OUTPUT_LINES);
                    for l in lines {
                        out.push_str(format!("{c}   {l}").trim_end());
                        out.push('\n');
                    }
                    if more > 0 {
                        out.push_str(&format!("{c}   … {more} more lines\n"));
                    }
                }
            }
            NbCellKind::Markdown => {
                out.push_str(&format!("{c} %% [markdown]\n"));
                push_lines(&mut out, &cell.source, &format!("{c} "));
            }
            NbCellKind::Raw => {
                out.push_str(&format!("{c} %% [raw]\n"));
                push_lines(&mut out, &cell.source, &format!("{c} "));
            }
        }
    }
    out
}

fn push_lines(out: &mut String, text: &str, prefix: &str) {
    for line in super::text::lines(text) {
        out.push_str(format!("{prefix}{line}").trim_end());
        out.push('\n');
    }
}

/// Removes ANSI escape sequences (CSI and OSC).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The Script view's diff, marked `derived_text` (its lines are not the
/// file's, so they cannot be selected). `Empty` when the scripts match.
pub fn script_diff(previous: Option<&NotebookBlob>, current: Option<&NotebookBlob>) -> Diff {
    let old = previous.map(script);
    let new = current.map(script);
    mark_derived(crate::pdf::line_diff(old.as_deref(), new.as_deref()))
}

/// Marks a text diff made from something other than the file's lines.
pub fn mark_derived(diff: Diff) -> Diff {
    match diff {
        Diff::Text {
            hunks,
            mut warnings,
        } => {
            warnings.derived_text = true;
            Diff::Text { hunks, warnings }
        }
        Diff::LargeText {
            hunks,
            mut warnings,
        } => {
            warnings.derived_text = true;
            Diff::LargeText { hunks, warnings }
        }
        other => other,
    }
}
