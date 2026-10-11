//! Rich Text Format (RTF 1.9.1): the body text with character formatting,
//! paragraphs, outline-level headings, list items, tables and PNG / JPEG
//! pictures. Font, colour and style tables, headers, footers, footnotes
//! and field instructions are skipped.

use corvene_models::{DocBlob, DocBlock, DocKind, DocRun, ListMark, RunStyle};

use super::{Builder, push_run, trim_runs};

/// Group nesting deeper than this is not read.
const MAX_DEPTH: usize = 512;

/// Destinations whose text is not part of the body.
const SKIPPED: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "footnote",
    "fldinst",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "xmlnstbl",
    "themedata",
    "colorschememapping",
    "datastore",
    "latentstyles",
    "filetbl",
    "revtbl",
    "pgdsctbl",
    "object",
    "objdata",
    "annotation",
    "atnid",
    "atnauthor",
    "bkmkstart",
    "bkmkend",
    "nonshppict",
    "shpinst",
    "mmathPr",
    "wgrffmtfilter",
    "xe",
    "tc",
    "template",
    "operator",
    "title",
    "author",
    "company",
    "comment",
    "doccomm",
    "keywords",
    "subject",
    "creatim",
    "revtim",
    "printim",
    "buptim",
    "userprops",
    "docvar",
];

#[derive(Clone, Copy, Default)]
struct State {
    style: RunStyle,
    /// Text in this group is not shown.
    skip: bool,
    /// Bytes a `\uN` replaces.
    uc: usize,
    /// In `\listtext` / `\pntext` (the list item's own number).
    list_text: bool,
    /// In a `\pict`.
    pict: bool,
}

/// What the current paragraph is.
#[derive(Clone, Copy, Default)]
struct Para {
    outline: Option<u8>,
    list: bool,
    in_table: bool,
}

struct Rtf {
    out: Builder,
    runs: Vec<DocRun>,
    para: Para,
    list_text: String,
    pict: Vec<u8>,
    pict_png: bool,
    /// Table rows gathered until the table ends.
    rows: Vec<Vec<Vec<DocRun>>>,
    row: Vec<Vec<DocRun>>,
    cell: Vec<DocRun>,
    /// `\ansicpg`
    encoding: &'static encoding_rs::Encoding,
    /// The last list number per level (`\ilvl`).
    counters: [u32; 9],
    level: u8,
}

pub(super) fn read(bytes: &[u8]) -> Result<DocBlob, String> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err("not RTF".into());
    }
    let mut rtf = Rtf {
        out: Builder::new(DocKind::Text),
        runs: Vec::new(),
        para: Para::default(),
        list_text: String::new(),
        pict: Vec::new(),
        pict_png: false,
        rows: Vec::new(),
        row: Vec::new(),
        cell: Vec::new(),
        encoding: encoding_rs::WINDOWS_1252,
        counters: [0; 9],
        level: 0,
    };
    rtf.parse(bytes)?;
    rtf.end_paragraph();
    rtf.end_table();
    Ok(rtf.out.finish())
}

impl Rtf {
    fn parse(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut stack: Vec<State> = Vec::new();
        let mut state = State {
            uc: 1,
            ..State::default()
        };
        // bytes of `\'hh` escapes, decoded together
        let mut pending: Vec<u8> = Vec::new();
        // characters still to skip after a `\uN`
        let mut skip_chars = 0usize;
        // a group just opened: its first control word may name a destination
        let mut group_start = false;
        // after `\*`: Word's `{\*\shppict …}` holds the picture to show
        let mut star = false;
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match b {
                b'{' => {
                    self.flush_pending(&mut pending, &state);
                    if stack.len() >= MAX_DEPTH {
                        return Err("too deep".into());
                    }
                    stack.push(state);
                    group_start = true;
                    skip_chars = 0;
                    i += 1;
                }
                b'}' => {
                    self.flush_pending(&mut pending, &state);
                    if state.pict && !state.skip && !stack.last().is_some_and(|s| s.pict) {
                        self.end_pict();
                    }
                    state = stack.pop().unwrap_or_default();
                    group_start = false;
                    i += 1;
                }
                b'\\' => {
                    i += 1;
                    let Some(&c) = bytes.get(i) else { break };
                    if c.is_ascii_alphabetic() {
                        let start = i;
                        while i < bytes.len() && bytes[i].is_ascii_alphabetic() && i - start < 32 {
                            i += 1;
                        }
                        let word = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                        let num_start = i;
                        if i < bytes.len() && bytes[i] == b'-' {
                            i += 1;
                        }
                        while i < bytes.len() && bytes[i].is_ascii_digit() && i - num_start < 11 {
                            i += 1;
                        }
                        let param = std::str::from_utf8(&bytes[num_start..i])
                            .ok()
                            .and_then(|p| p.parse::<i64>().ok());
                        if i < bytes.len() && bytes[i] == b' ' {
                            i += 1;
                        }
                        self.flush_pending(&mut pending, &state);
                        let first = std::mem::take(&mut group_start);
                        if std::mem::take(&mut star) && word == "shppict" {
                            state.skip = stack.last().is_some_and(|s| s.skip);
                            continue;
                        }
                        if first && SKIPPED.contains(&word) {
                            state.skip = true;
                            continue;
                        }
                        if skip_chars > 0 {
                            skip_chars -= 1;
                            continue;
                        }
                        self.control(word, param, &mut state, &mut skip_chars);
                    } else if c == b'\'' {
                        let hex = bytes
                            .get(i + 1..i + 3)
                            .and_then(|h| std::str::from_utf8(h).ok());
                        i += 3;
                        group_start = false;
                        if skip_chars > 0 {
                            skip_chars -= 1;
                            continue;
                        }
                        if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                            pending.push(b);
                        }
                    } else {
                        i += 1;
                        let first = std::mem::take(&mut group_start);
                        self.flush_pending(&mut pending, &state);
                        match c {
                            // `{\*\dest …}`: an optional destination, skipped
                            b'*' if first => {
                                state.skip = true;
                                star = true;
                            }
                            b'\\' | b'{' | b'}' => self.text(&(c as char).to_string(), &state),
                            b'~' => self.text("\u{a0}", &state),
                            b'_' => self.text("\u{2011}", &state),
                            b'\n' | b'\r' => self.paragraph_end(),
                            _ => {}
                        }
                    }
                }
                b'\r' | b'\n' => i += 1,
                _ => {
                    group_start = false;
                    let start = i;
                    while i < bytes.len()
                        && !matches!(bytes[i], b'{' | b'}' | b'\\' | b'\r' | b'\n')
                    {
                        i += 1;
                    }
                    let mut chunk = &bytes[start..i];
                    while skip_chars > 0 && !chunk.is_empty() {
                        chunk = &chunk[1..];
                        skip_chars -= 1;
                    }
                    if state.pict && !state.skip {
                        self.pict
                            .extend(chunk.iter().copied().filter(u8::is_ascii_hexdigit));
                    } else {
                        pending.extend_from_slice(chunk);
                    }
                }
            }
        }
        Ok(())
    }

    fn flush_pending(&mut self, pending: &mut Vec<u8>, state: &State) {
        if pending.is_empty() {
            return;
        }
        let (text, _, _) = self.encoding.decode(pending);
        let text = text.into_owned();
        pending.clear();
        self.text(&text, state);
    }

    fn text(&mut self, text: &str, state: &State) {
        if state.skip || state.pict {
            return;
        }
        if state.list_text {
            self.list_text.push_str(text);
            return;
        }
        let target = if self.para.in_table {
            &mut self.cell
        } else {
            &mut self.runs
        };
        push_run(target, text, state.style, None);
    }

    fn control(
        &mut self,
        word: &str,
        param: Option<i64>,
        state: &mut State,
        skip_chars: &mut usize,
    ) {
        let flag = param.is_none_or(|p| p != 0);
        match word {
            "ansicpg" => {
                if let Some(enc) = param.and_then(code_page) {
                    self.encoding = enc;
                }
            }
            "pict" if !state.skip => {
                state.pict = true;
                self.pict.clear();
                self.pict_png = false;
            }
            "pngblip" => self.pict_png = true,
            "jpegblip" => self.pict_png = true,
            // a picture's size and other words before its data
            _ if state.pict => {}
            "listtext" | "pntext" => state.list_text = true,
            "b" => state.style.bold = flag,
            "i" => state.style.italic = flag,
            "ul" | "uld" | "uldb" | "ulw" | "ulwave" | "uldash" | "ulth" => {
                state.style.underline = flag
            }
            "ulnone" => state.style.underline = false,
            "strike" | "striked" => state.style.strike = flag,
            "super" => {
                state.style.superscript = flag;
                state.style.subscript = false;
            }
            "sub" => {
                state.style.subscript = flag;
                state.style.superscript = false;
            }
            "nosupersub" => {
                state.style.superscript = false;
                state.style.subscript = false;
            }
            "plain" => state.style = RunStyle::default(),
            "uc" => state.uc = param.unwrap_or(1).clamp(0, 8) as usize,
            "u" => {
                if let Some(p) = param {
                    let code = if p < 0 { p + 65_536 } else { p } as u32;
                    if let Some(c) = char::from_u32(code) {
                        self.text(&c.to_string(), state);
                    }
                    *skip_chars = state.uc;
                }
            }
            "par" => self.paragraph_end(),
            "line" => self.text("\n", state),
            "tab" => self.text("\t", state),
            "page" => {
                self.end_paragraph();
                self.end_table();
                self.out.push(DocBlock::Break);
            }
            "pard" => {
                self.para.outline = None;
                self.para.list = false;
                self.para.in_table = false;
            }
            "outlinelevel" => {
                self.para.outline = param.and_then(|p| u8::try_from(p).ok()).filter(|l| *l < 9)
            }
            "ls" => self.para.list = true,
            "ilvl" => self.level = param.and_then(|p| u8::try_from(p).ok()).unwrap_or(0).min(8),
            "intbl" => self.para.in_table = true,
            "cell" => {
                let cell = trim_runs(std::mem::take(&mut self.cell));
                self.row.push(cell);
            }
            "row" => {
                let row = std::mem::take(&mut self.row);
                self.rows.push(row);
            }
            "emdash" => self.text("—", state),
            "endash" => self.text("–", state),
            "lquote" => self.text("‘", state),
            "rquote" => self.text("’", state),
            "ldblquote" => self.text("“", state),
            "rdblquote" => self.text("”", state),
            "bullet" => self.text("•", state),
            "emspace" => self.text("\u{2003}", state),
            "enspace" => self.text("\u{2002}", state),
            _ => {}
        }
    }

    /// `\par`: a paragraph ends (inside a cell, a line break).
    fn paragraph_end(&mut self) {
        if self.para.in_table {
            push_run(&mut self.cell, "\n", RunStyle::default(), None);
            return;
        }
        self.end_paragraph();
    }

    fn end_paragraph(&mut self) {
        let runs = trim_runs(std::mem::take(&mut self.runs));
        let list_text = std::mem::take(&mut self.list_text);
        if runs.is_empty() {
            return;
        }
        self.end_table();
        if let Some(level) = self.para.outline
            && !self.para.list
        {
            self.out.push(DocBlock::Heading {
                level: (level + 1).min(6),
                runs,
            });
            return;
        }
        let list = self.para.list.then(|| {
            let level = self.level;
            for c in self.counters.iter_mut().skip(usize::from(level) + 1) {
                *c = 0;
            }
            self.counters[usize::from(level)] += 1;
            ListMark {
                level,
                ordered: list_text.chars().any(|c| c.is_ascii_alphanumeric()),
                number: self.counters[usize::from(level)],
            }
        });
        if list.is_none() {
            self.counters = [0; 9];
        }
        self.out.push(DocBlock::Paragraph {
            runs,
            list,
            quote: false,
        });
    }

    fn end_table(&mut self) {
        if !self.row.is_empty() {
            let row = std::mem::take(&mut self.row);
            self.rows.push(row);
        }
        if self.rows.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.rows);
        self.out.push(DocBlock::Table { rows });
    }

    fn end_pict(&mut self) {
        let hex = std::mem::take(&mut self.pict);
        if !self.pict_png {
            self.out.push(DocBlock::Image {
                index: None,
                alt: String::new(),
            });
            return;
        }
        let bytes: Vec<u8> = hex
            .as_chunks::<2>()
            .0
            .iter()
            .filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
            .collect();
        self.end_paragraph();
        let index = self.out.image("picture", Some(bytes));
        self.out.push(DocBlock::Image {
            index,
            alt: String::new(),
        });
    }
}

/// `\ansicpgN` → its encoding.
fn code_page(n: i64) -> Option<&'static encoding_rs::Encoding> {
    let label = match n {
        932 => "shift_jis".to_string(),
        936 => "gbk".to_string(),
        949 => "euc-kr".to_string(),
        950 => "big5".to_string(),
        874 => "windows-874".to_string(),
        10000 => "macintosh".to_string(),
        65001 => "utf-8".to_string(),
        1250..=1258 => format!("windows-{n}"),
        _ => return None,
    };
    encoding_rs::Encoding::for_label(label.as_bytes())
}
