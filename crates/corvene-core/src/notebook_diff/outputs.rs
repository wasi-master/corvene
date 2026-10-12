//! Outputs of code cells: which MIME type of a bundle to draw, how two
//! versions' output lists pair up, and pandas DataFrame HTML read back into
//! rows so a changed table shows its changed cells.

use std::hash::{DefaultHasher, Hash, Hasher};

use corvene_models::{NbData, NbOutput, NbOutputKind};

use crate::csv_diff::{Record, TableDiff, records_diff};

/// How to draw one entry of an output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Render {
    /// Terminal text with ANSI escapes (streams, tracebacks).
    Ansi {
        text: String,
        stderr: bool,
    },
    /// Plain text (`text/plain`).
    Text(String),
    /// A raster image (`image/png`, …), base64.
    Image {
        mime: String,
        base64: String,
    },
    Svg(String),
    /// An HTML table (a pandas DataFrame), header row first.
    Table(Vec<Vec<String>>),
    /// Other HTML, drawn with the Markdown renderer's HTML subset.
    Html(String),
    Markdown(String),
    /// TeX math (`text/latex`, without its `$` delimiters).
    Latex(String),
    Json(String),
    /// A PDF, base64.
    Pdf(String),
    /// Output the view cannot run (widgets, JavaScript, interactive charts):
    /// a description, and the MIME type.
    Placeholder {
        mime: String,
        what: &'static str,
    },
}

impl Render {
    /// Whether a text diff of the two renderings makes sense.
    pub fn text(&self) -> Option<&str> {
        match self {
            Render::Ansi { text, .. }
            | Render::Text(text)
            | Render::Html(text)
            | Render::Markdown(text)
            | Render::Latex(text)
            | Render::Json(text)
            | Render::Svg(text) => Some(text),
            _ => None,
        }
    }
}

/// One way to show an output: the MIME type (or `stdout` / `stderr` /
/// `error`) and the drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendering {
    pub mime: String,
    pub render: Render,
}

/// The renderings of an output, best first (the chips of the view list
/// them all).
pub fn renderings(output: &NbOutput) -> Vec<Rendering> {
    match output.kind {
        NbOutputKind::Stream => vec![Rendering {
            mime: output.name.clone(),
            render: Render::Ansi {
                text: output.text.clone(),
                stderr: output.name == "stderr",
            },
        }],
        NbOutputKind::Error => {
            let text = if output.text.trim().is_empty() {
                format!("{}: {}", output.name, output.evalue)
            } else {
                output.text.clone()
            };
            vec![Rendering {
                mime: "error".into(),
                render: Render::Ansi { text, stderr: true },
            }]
        }
        NbOutputKind::DisplayData | NbOutputKind::ExecuteResult => {
            let mut out: Vec<(u8, Rendering)> = output
                .data
                .iter()
                .filter_map(|m| {
                    let (rank, render) = render_mime(&m.mime, &m.data)?;
                    Some((
                        rank,
                        Rendering {
                            mime: m.mime.clone(),
                            render,
                        },
                    ))
                })
                .collect();
            out.sort_by_key(|(rank, _)| *rank);
            out.into_iter().map(|(_, r)| r).collect()
        }
    }
}

/// The drawing of one MIME entry and its rank (lower is preferred).
fn render_mime(mime: &str, data: &NbData) -> Option<(u8, Render)> {
    let text = || data.text().to_string();
    Some(match mime {
        "text/html" => match html_table(data.text()) {
            Some(rows) if is_dataframe(data.text()) || !rows.is_empty() => (0, Render::Table(rows)),
            _ if script_only(data.text()) => (
                90,
                Render::Placeholder {
                    mime: mime.into(),
                    what: "Interactive HTML output",
                },
            ),
            _ => (40, Render::Html(text())),
        },
        "text/markdown" => (5, Render::Markdown(text())),
        "text/latex" => (10, Render::Latex(latex_body(data.text()))),
        "image/svg+xml" => (20, Render::Svg(text())),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp" => (
            21,
            Render::Image {
                mime: mime.into(),
                base64: text(),
            },
        ),
        "application/pdf" => (30, Render::Pdf(text())),
        "application/json" => (50, Render::Json(text())),
        "text/plain" => (60, Render::Text(text())),
        "application/vnd.jupyter.widget-view+json" => (
            80,
            Render::Placeholder {
                mime: mime.into(),
                what: "Jupyter widget",
            },
        ),
        "application/javascript" | "text/javascript" => (
            85,
            Render::Placeholder {
                mime: mime.into(),
                what: "JavaScript output",
            },
        ),
        m if m.starts_with("application/vnd.plotly") => (
            82,
            Render::Placeholder {
                mime: mime.into(),
                what: "Plotly chart",
            },
        ),
        m if m.starts_with("application/vnd.vegalite") || m.starts_with("application/vnd.vega") => {
            (
                82,
                Render::Placeholder {
                    mime: mime.into(),
                    what: "Vega chart",
                },
            )
        }
        m if m.ends_with("+json") => (55, Render::Json(text())),
        m if m.starts_with("text/") => (65, Render::Text(text())),
        _ => (
            95,
            Render::Placeholder {
                mime: mime.into(),
                what: "Output",
            },
        ),
    })
}

/// `text/latex` as the math engine takes it: `$…$`, `$$…$$`, `\[…\]` and
/// `\(…\)` delimiters removed.
pub fn latex_body(text: &str) -> String {
    let t = text.trim();
    for (open, close) in [("$$", "$$"), ("\\[", "\\]"), ("\\(", "\\)"), ("$", "$")] {
        if let Some(inner) = t.strip_prefix(open).and_then(|r| r.strip_suffix(close))
            && !inner.is_empty()
        {
            return inner.trim().to_string();
        }
    }
    t.to_string()
}

/// An output's identity without its execution count.
pub fn output_key(output: &NbOutput) -> u64 {
    let mut h = DefaultHasher::new();
    output.kind.hash(&mut h);
    output.name.hash(&mut h);
    output.text.hash(&mut h);
    output.evalue.hash(&mut h);
    output.data.hash(&mut h);
    output.metadata.hash(&mut h);
    h.finish()
}

/// An output's shape: kind, stream and MIME types (two outputs of the same
/// shape are one output changed).
fn output_shape(output: &NbOutput) -> u64 {
    let mut h = DefaultHasher::new();
    output.kind.hash(&mut h);
    if output.kind == NbOutputKind::Stream {
        output.name.hash(&mut h);
    }
    let mut mimes: Vec<&str> = output.data.iter().map(|m| m.mime.as_str()).collect();
    mimes.sort_unstable();
    mimes.hash(&mut h);
    h.finish()
}

/// How an output changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStatus {
    Same,
    Added,
    Deleted,
    Modified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputPair {
    pub previous: Option<usize>,
    pub current: Option<usize>,
    pub status: OutputStatus,
}

/// The outputs of two versions of a cell, paired: equal outputs, then
/// outputs of the same shape within a changed stretch.
pub fn output_pairs(old: &[NbOutput], new: &[NbOutput]) -> Vec<OutputPair> {
    let ok: Vec<u64> = old.iter().map(output_key).collect();
    let nk: Vec<u64> = new.iter().map(output_key).collect();
    let mut out = Vec::new();
    let pair = |previous, current, status| OutputPair {
        previous,
        current,
        status,
    };
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &ok, &nk);
    let mut ops = ops.into_iter().peekable();
    while let Some(op) = ops.next() {
        let (tag, o, n) = op.as_tag_tuple();
        if tag == similar::DiffTag::Equal {
            out.extend(
                o.zip(n)
                    .map(|(i, j)| pair(Some(i), Some(j), OutputStatus::Same)),
            );
            continue;
        }
        let (mut olds, mut news): (Vec<usize>, Vec<usize>) = (o.collect(), n.collect());
        while let Some(next) = ops.peek() {
            let (tag, o, n) = next.as_tag_tuple();
            if tag == similar::DiffTag::Equal {
                break;
            }
            olds.extend(o);
            news.extend(n);
            ops.next();
        }
        let mut j = 0;
        for &i in &olds {
            let shape = output_shape(&old[i]);
            match (j..news.len()).find(|&m| output_shape(&new[news[m]]) == shape) {
                Some(m) => {
                    out.extend(
                        news[j..m]
                            .iter()
                            .map(|&k| pair(None, Some(k), OutputStatus::Added)),
                    );
                    out.push(pair(Some(i), Some(news[m]), OutputStatus::Modified));
                    j = m + 1;
                }
                None => out.push(pair(Some(i), None, OutputStatus::Deleted)),
            }
        }
        out.extend(
            news[j..]
                .iter()
                .map(|&k| pair(None, Some(k), OutputStatus::Added)),
        );
    }
    out
}

/// A pandas DataFrame (`<table class="dataframe">`).
pub fn is_dataframe(html: &str) -> bool {
    html.contains("class=\"dataframe") || html.contains("class='dataframe")
}

/// HTML that is all script (a chart library's loader): nothing to read.
fn script_only(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("<script") && visible_text(html).trim().len() < 16
}

fn visible_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let lower = html.to_ascii_lowercase();
    let mut skip_until: Option<&str> = None;
    let mut i = 0;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if let Some(end) = skip_until {
            if lower[i..].starts_with(end) {
                i += end.len();
                skip_until = None;
            } else {
                i += 1;
            }
            continue;
        }
        if lower[i..].starts_with("<script") {
            skip_until = Some("</script>");
            continue;
        }
        if lower[i..].starts_with("<style") {
            skip_until = Some("</style>");
            continue;
        }
        let c = bytes[i];
        if c == b'<' {
            in_tag = true;
        } else if c == b'>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c as char);
        }
        i += 1;
    }
    out
}

/// The first `<table>` of an HTML fragment as rows of cell text (header
/// rows first; `colspan` repeats a cell's text as empty cells, `rowspan`
/// repeats it down, as pandas writes MultiIndex tables). `None` without a
/// table.
pub fn html_table(html: &str) -> Option<Vec<Vec<String>>> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<table")?;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Option<Vec<String>> = None;
    let mut cell: Option<(String, usize, usize)> = None;
    // (column, remaining rows, text) of rowspans carried down
    let mut carried: Vec<(usize, usize, String)> = Vec::new();
    let mut depth = 0usize;
    let mut i = start;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let end = match html[i..].find('>') {
                Some(e) => i + e,
                None => break,
            };
            let tag = &lower[i + 1..end];
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let closing = tag.starts_with('/');
            match (name.as_str(), closing) {
                ("table", false) => depth += 1,
                ("table", true) => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ if depth != 1 => {}
                ("tr", false) => {
                    if let Some(r) = row.take() {
                        rows.push(r);
                    }
                    row = Some(Vec::new());
                    fill_carried(&mut row, &mut carried);
                }
                ("tr", true) => {
                    finish_cell(&mut cell, &mut row, &mut carried);
                    if let Some(r) = row.take() {
                        rows.push(r);
                    }
                }
                ("td" | "th", false) => {
                    finish_cell(&mut cell, &mut row, &mut carried);
                    let span = |attr: &str| {
                        attr_value(tag, attr)
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(1)
                            .clamp(1, 1000)
                    };
                    cell = Some((String::new(), span("colspan"), span("rowspan")));
                }
                ("td" | "th", true) => finish_cell(&mut cell, &mut row, &mut carried),
                ("br", _) => {
                    if let Some((text, _, _)) = &mut cell {
                        text.push(' ');
                    }
                }
                _ => {}
            }
            i = end + 1;
            continue;
        }
        let next = html[i..].find('<').map_or(bytes.len(), |n| i + n);
        if depth == 1
            && let Some((text, _, _)) = &mut cell
        {
            text.push_str(&decode_entities(&html[i..next]));
        }
        i = next;
    }
    finish_cell(&mut cell, &mut row, &mut carried);
    if let Some(r) = row.take() {
        rows.push(r);
    }
    rows.retain(|r| !r.is_empty());
    if rows.is_empty() {
        return None;
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    for r in &mut rows {
        r.resize(width, String::new());
    }
    Some(rows)
}

fn fill_carried(row: &mut Option<Vec<String>>, carried: &mut Vec<(usize, usize, String)>) {
    let Some(r) = row else { return };
    carried.sort_by_key(|c| c.0);
    for (col, left, text) in carried.iter_mut() {
        if *left == 0 {
            continue;
        }
        while r.len() < *col {
            r.push(String::new());
        }
        if r.len() == *col {
            r.push(text.clone());
        }
        *left -= 1;
    }
    carried.retain(|c| c.1 > 0);
}

fn finish_cell(
    cell: &mut Option<(String, usize, usize)>,
    row: &mut Option<Vec<String>>,
    carried: &mut Vec<(usize, usize, String)>,
) {
    let Some((text, colspan, rowspan)) = cell.take() else {
        return;
    };
    let r = row.get_or_insert_with(Vec::new);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let col = r.len();
    if rowspan > 1 {
        carried.push((col, rowspan - 1, text.clone()));
    }
    r.push(text);
    r.extend(std::iter::repeat_n(String::new(), colspan - 1));
}

fn attr_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// HTML character references.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A cell-level diff of two tables (header row first; `None` for a side
/// without the table).
pub fn table_diff(old: Option<&[Vec<String>]>, new: Option<&[Vec<String>]>) -> Option<TableDiff> {
    let records = |rows: Option<&[Vec<String>]>| -> Vec<Record> {
        rows.unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, r)| Record {
                line: i as u32 + 1,
                cells: r.clone(),
            })
            .collect()
    };
    records_diff(records(old), records(new)).ok()
}

/// Standard or URL-safe base64, white space ignored; `None` when it is not
/// base64.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in text.as_bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod base64_tests {
    #[test]
    fn decodes() {
        assert_eq!(super::decode_base64("aGVs\nbG8=").unwrap(), b"hello");
        assert_eq!(super::decode_base64("aGk").unwrap(), b"hi");
        assert!(super::decode_base64("a$b").is_none());
    }
}
