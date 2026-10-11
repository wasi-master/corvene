//! OpenDocument Text and Presentation files (ODF 1.3): a zip package's
//! `content.xml` + `styles.xml`, or one flat `.fodt` / `.fodp` XML file.

use std::collections::HashMap;

use corvene_models::{DocBlob, DocBlock, DocKind, DocRun, ListMark, RunStyle};
use roxmltree::{Document, Node};

use super::{
    Builder, Package, attr, child, collapse_ws, name, parse_xml, push_run, trim_runs, xml_text,
};

pub(super) fn read_package(bytes: &[u8]) -> Result<DocBlob, String> {
    let mut pkg = Package::open(bytes)?;
    let content = pkg.xml("content.xml").ok_or("no content.xml")?;
    let styles = pkg.xml("styles.xml");
    let content = parse_xml(&content)?;
    let styles = styles.as_deref().and_then(|s| parse_xml(s).ok());
    read_doc(&content, styles.as_ref(), Some(&mut pkg))
}

pub(super) fn read_flat(bytes: &[u8]) -> Result<DocBlob, String> {
    let text = xml_text(bytes.to_vec()).ok_or("not UTF-8")?;
    let doc = parse_xml(&text)?;
    read_doc(&doc, None, None)
}

/// Text formatting a style sets (`None`: inherited).
#[derive(Clone, Default)]
struct Props {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    position: Option<i8>,
    parent: Option<String>,
    hidden: bool,
}

struct Odf<'p, 'b> {
    pkg: Option<&'p mut Package<'b>>,
    styles: HashMap<String, Props>,
    /// list style → level (1-based) → ordered
    lists: HashMap<String, HashMap<u8, bool>>,
    out: Builder,
    notes: bool,
}

fn read_doc(
    content: &Document,
    styles: Option<&Document>,
    pkg: Option<&mut Package>,
) -> Result<DocBlob, String> {
    let mut props = HashMap::new();
    let mut lists = HashMap::new();
    for doc in styles.into_iter().chain(std::iter::once(content)) {
        collect_styles(doc, &mut props, &mut lists);
    }
    let body = content
        .root_element()
        .children()
        .find(|n| name(*n) == "body")
        .ok_or("no office:body")?;
    let (kind, top) = if let Some(text) = child(body, "text") {
        (DocKind::Text, text)
    } else if let Some(pres) = child(body, "presentation") {
        (DocKind::Presentation, pres)
    } else {
        return Err("neither text nor presentation".into());
    };
    let mut odf = Odf {
        pkg,
        styles: props,
        lists,
        out: Builder::new(kind),
        notes: false,
    };
    match kind {
        DocKind::Presentation => {
            for page in top.children().filter(|n| name(*n) == "page") {
                odf.page(page);
            }
        }
        _ => odf.blocks(top, None),
    }
    Ok(odf.out.finish())
}

fn collect_styles(
    doc: &Document,
    props: &mut HashMap<String, Props>,
    lists: &mut HashMap<String, HashMap<u8, bool>>,
) {
    for node in doc.descendants() {
        match name(node) {
            "style" => {
                let Some(style_name) = attr(node, "name") else {
                    continue;
                };
                let mut p = Props {
                    parent: attr(node, "parent-style-name").map(str::to_string),
                    ..Props::default()
                };
                if let Some(tp) = child(node, "text-properties") {
                    if let Some(w) = attr(tp, "font-weight") {
                        p.bold = Some(w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
                    }
                    if let Some(s) = attr(tp, "font-style") {
                        p.italic = Some(s != "normal");
                    }
                    if let Some(u) = attr(tp, "text-underline-style") {
                        p.underline = Some(u != "none");
                    }
                    if let Some(l) = attr(tp, "text-line-through-style") {
                        p.strike = Some(l != "none");
                    }
                    if let Some(pos) = attr(tp, "text-position") {
                        let first = pos.split_whitespace().next().unwrap_or("");
                        p.position = Some(if first == "super" {
                            1
                        } else if first == "sub" {
                            -1
                        } else {
                            let n = first.trim_end_matches('%').parse::<f32>().unwrap_or(0.0);
                            if n > 0.0 {
                                1
                            } else if n < 0.0 {
                                -1
                            } else {
                                0
                            }
                        });
                    }
                }
                if let Some(dp) = child(node, "drawing-page-properties") {
                    p.hidden = attr(dp, "visibility") == Some("hidden");
                }
                props.insert(style_name.to_string(), p);
            }
            "list-style" => {
                let Some(style_name) = attr(node, "name") else {
                    continue;
                };
                let levels = node
                    .children()
                    .filter_map(|l| {
                        let level = attr(l, "level")?.parse::<u8>().ok()?;
                        Some((level, name(l) == "list-level-style-number"))
                    })
                    .collect();
                lists.insert(style_name.to_string(), levels);
            }
            _ => {}
        }
    }
}

/// Where a paragraph sits in a list.
#[derive(Clone)]
struct ListCtx {
    style: Option<String>,
    level: u8,
}

impl Odf<'_, '_> {
    fn push(&mut self, block: DocBlock) {
        if self.notes {
            self.out.push_note(block);
        } else {
            self.out.push(block);
        }
    }

    /// The formatting of a style, its parents' under it.
    fn style(&self, base: RunStyle, style_name: Option<&str>) -> RunStyle {
        let mut chain = Vec::new();
        let mut cur = style_name.map(str::to_string);
        while let Some(n) = cur {
            if chain.len() > 16 {
                break;
            }
            let Some(p) = self.styles.get(&n) else {
                break;
            };
            chain.push(p);
            cur = p.parent.clone();
        }
        let mut style = base;
        for p in chain.into_iter().rev() {
            if let Some(b) = p.bold {
                style.bold = b;
            }
            if let Some(i) = p.italic {
                style.italic = i;
            }
            if let Some(u) = p.underline {
                style.underline = u;
            }
            if let Some(s) = p.strike {
                style.strike = s;
            }
            if let Some(pos) = p.position {
                style.superscript = pos > 0;
                style.subscript = pos < 0;
            }
        }
        style
    }

    fn blocks(&mut self, parent: Node, list: Option<ListCtx>) {
        for node in parent.children() {
            match name(node) {
                "h" => {
                    let level = attr(node, "outline-level")
                        .and_then(|l| l.parse::<u8>().ok())
                        .unwrap_or(1)
                        .clamp(1, 6);
                    let (runs, frames) = self.paragraph_runs(node);
                    self.push(DocBlock::Heading { level, runs });
                    self.frames(frames);
                }
                "p" => {
                    let (runs, frames) = self.paragraph_runs(node);
                    self.push(DocBlock::Paragraph {
                        runs,
                        list: None,
                        quote: false,
                    });
                    self.frames(frames);
                }
                "list" => self.list(node, list.as_ref()),
                "table" => {
                    let rows = self.table(node);
                    self.push(DocBlock::Table { rows });
                }
                "section" | "index-body" | "table-of-content" | "illustration-index"
                | "table-index" | "object-index" | "user-index" | "alphabetical-index"
                | "bibliography" | "text-box" => self.blocks(node, list.clone()),
                "frame" => self.frames(vec![node]),
                // tracked deletions, forms, sequence declarations, …
                _ => {}
            }
        }
    }

    fn list(&mut self, node: Node, outer: Option<&ListCtx>) {
        let style = attr(node, "style-name")
            .map(str::to_string)
            .or_else(|| outer.and_then(|o| o.style.clone()));
        let level = outer.map_or(0, |o| o.level + 1).min(8);
        let ctx = ListCtx {
            style: style.clone(),
            level,
        };
        let ordered = style
            .as_deref()
            .and_then(|s| self.lists.get(s))
            .and_then(|levels| levels.get(&(level + 1)).copied())
            .unwrap_or(false);
        let mut number = 0;
        for item in node
            .children()
            .filter(|n| matches!(name(*n), "list-item" | "list-header"))
        {
            let header = name(item) == "list-header";
            if !header {
                number += 1;
            }
            let mut first = !header;
            for part in item.children() {
                match name(part) {
                    "p" | "h" => {
                        let (runs, frames) = self.paragraph_runs(part);
                        let mark = first.then_some(ListMark {
                            level,
                            ordered,
                            number,
                        });
                        first = false;
                        self.push(DocBlock::Paragraph {
                            runs,
                            list: mark,
                            quote: false,
                        });
                        self.frames(frames);
                    }
                    "list" => self.list(part, Some(&ctx)),
                    "table" => {
                        let rows = self.table(part);
                        self.push(DocBlock::Table { rows });
                    }
                    _ => {}
                }
            }
        }
    }

    fn paragraph_runs<'a, 'i>(&self, p: Node<'a, 'i>) -> (Vec<DocRun>, Vec<Node<'a, 'i>>) {
        let base = self.style(RunStyle::default(), attr(p, "style-name"));
        let mut runs = Vec::new();
        let mut frames = Vec::new();
        let mut after_space = true;
        self.inline(p, base, None, &mut runs, &mut frames, &mut after_space);
        (trim_runs(runs), frames)
    }

    #[allow(clippy::too_many_arguments)]
    fn inline<'a, 'i>(
        &self,
        parent: Node<'a, 'i>,
        style: RunStyle,
        link: Option<&str>,
        runs: &mut Vec<DocRun>,
        frames: &mut Vec<Node<'a, 'i>>,
        after_space: &mut bool,
    ) {
        for node in parent.children() {
            if node.is_text() {
                let text = collapse_ws(node.text().unwrap_or(""), after_space);
                push_run(runs, &text, style, link);
                continue;
            }
            match name(node) {
                "span" => {
                    let style = self.style(style, attr(node, "style-name"));
                    self.inline(node, style, link, runs, frames, after_space);
                }
                "a" => {
                    let href = attr(node, "href");
                    self.inline(node, style, href.or(link), runs, frames, after_space);
                }
                "s" => {
                    let n = attr(node, "c")
                        .and_then(|c| c.parse::<usize>().ok())
                        .unwrap_or(1)
                        .min(64);
                    push_run(runs, &" ".repeat(n), style, link);
                    *after_space = true;
                }
                "tab" => {
                    push_run(runs, "\t", style, link);
                    *after_space = true;
                }
                "line-break" => {
                    push_run(runs, "\n", style, link);
                    *after_space = true;
                }
                "frame" => frames.push(node),
                "note"
                | "annotation"
                | "annotation-end"
                | "bookmark"
                | "bookmark-start"
                | "bookmark-end"
                | "soft-page-break"
                | "change"
                | "change-start"
                | "change-end"
                | "reference-mark"
                | "reference-mark-start"
                | "reference-mark-end"
                | "toc-mark"
                | "alphabetical-index-mark" => {}
                "" => {}
                // fields (page numbers, dates, …), ruby, meta: their text
                _ => self.inline(node, style, link, runs, frames, after_space),
            }
        }
    }

    /// Pictures and text boxes anchored in a paragraph, after it.
    fn frames(&mut self, frames: Vec<Node>) {
        for frame in frames {
            let alt = child(frame, "desc")
                .or_else(|| child(frame, "title"))
                .and_then(|d| d.text())
                .or_else(|| attr(frame, "name"))
                .unwrap_or("")
                .to_string();
            if let Some(image) = child(frame, "image") {
                let href = attr(image, "href").filter(|h| !h.contains(':'));
                let index = match (href, self.pkg.as_deref_mut()) {
                    (Some(href), Some(pkg)) => {
                        let bytes = pkg.bytes(href);
                        self.out.image(href, bytes)
                    }
                    _ => None,
                };
                self.push(DocBlock::Image { index, alt });
            } else if let Some(text_box) = child(frame, "text-box") {
                self.blocks(text_box, None);
            } else if let Some(table) = child(frame, "table") {
                let rows = self.table(table);
                self.push(DocBlock::Table { rows });
            }
        }
    }

    fn table(&self, table: Node) -> Vec<Vec<Vec<DocRun>>> {
        let mut rows = Vec::new();
        self.rows(table, &mut rows);
        while rows.last().is_some_and(|r: &Vec<Vec<DocRun>>| r.is_empty()) {
            rows.pop();
        }
        rows
    }

    fn rows(&self, parent: Node, rows: &mut Vec<Vec<Vec<DocRun>>>) {
        for node in parent.children() {
            match name(node) {
                "table-row" => {
                    let mut row = Vec::new();
                    for cell in node
                        .children()
                        .filter(|c| matches!(name(*c), "table-cell" | "covered-table-cell"))
                    {
                        let repeat = attr(cell, "number-columns-repeated")
                            .and_then(|r| r.parse::<usize>().ok())
                            .unwrap_or(1)
                            .min(64);
                        let runs = self.cell(cell);
                        for _ in 0..repeat {
                            row.push(runs.clone());
                        }
                    }
                    while row.last().is_some_and(|c: &Vec<DocRun>| c.is_empty()) {
                        row.pop();
                    }
                    let repeat = attr(node, "number-rows-repeated")
                        .and_then(|r| r.parse::<usize>().ok())
                        .unwrap_or(1)
                        .min(if row.is_empty() { 1 } else { 64 });
                    for _ in 0..repeat {
                        rows.push(row.clone());
                    }
                }
                "table-header-rows" | "table-rows" | "table-row-group" => self.rows(node, rows),
                _ => {}
            }
        }
    }

    fn cell(&self, cell: Node) -> Vec<DocRun> {
        let mut runs = Vec::new();
        for p in cell.descendants().filter(|n| matches!(name(*n), "p" | "h")) {
            let (r, _) = self.paragraph_runs(p);
            if r.is_empty() {
                continue;
            }
            if !runs.is_empty() {
                push_run(&mut runs, "\n", RunStyle::default(), None);
            }
            for run in r {
                push_run(&mut runs, &run.text, run.style, run.link.as_deref());
            }
        }
        runs
    }

    fn page(&mut self, page: Node) {
        let hidden = attr(page, "style-name")
            .and_then(|s| self.styles.get(s))
            .is_some_and(|p| p.hidden);
        self.out.section(String::new(), hidden);
        self.notes = false;
        let shapes: Vec<Node> = page.children().filter(|n| n.is_element()).collect();
        let (titles, rest): (Vec<Node>, Vec<Node>) = shapes
            .into_iter()
            .partition(|s| attr(*s, "class") == Some("title"));
        for shape in titles.into_iter().chain(rest) {
            self.shape(shape);
        }
    }

    fn shape(&mut self, shape: Node) {
        match name(shape) {
            "frame" => match attr(shape, "class") {
                Some("title") | Some("subtitle") => {
                    let level = if attr(shape, "class") == Some("title") {
                        1
                    } else {
                        2
                    };
                    let mut runs = Vec::new();
                    for p in shape.descendants().filter(|n| name(*n) == "p") {
                        let (r, _) = self.paragraph_runs(p);
                        if r.is_empty() {
                            continue;
                        }
                        if !runs.is_empty() {
                            push_run(&mut runs, "\n", RunStyle::default(), None);
                        }
                        for run in r {
                            push_run(&mut runs, &run.text, run.style, run.link.as_deref());
                        }
                    }
                    if level == 1 && !self.notes {
                        self.out
                            .name_section(&DocBlock::runs_text(&runs).replace('\n', " "));
                    }
                    self.push(DocBlock::Heading { level, runs });
                }
                Some("page-number" | "date-time" | "footer" | "header" | "page") => {}
                _ => self.frames(vec![shape]),
            },
            "g" => {
                for s in shape.children().filter(|n| n.is_element()) {
                    self.shape(s);
                }
            }
            "notes" => {
                self.notes = true;
                for s in shape
                    .children()
                    .filter(|n| name(*n) == "frame" && attr(*n, "class") == Some("notes"))
                {
                    self.frames(vec![s]);
                }
                self.notes = false;
            }
            "custom-shape" | "rect" | "ellipse" | "polygon" | "path" | "caption" => {
                self.blocks(shape, None)
            }
            _ => {}
        }
    }
}
