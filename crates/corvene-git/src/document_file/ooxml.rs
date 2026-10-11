//! Word (`word/document.xml`) and PowerPoint (`ppt/slides/slideN.xml`)
//! packages (ECMA-376).

use std::collections::HashMap;

use corvene_models::{DocBlock, DocKind, DocRun, ListMark, RunStyle};
use roxmltree::Node;

use super::{Builder, Package, attr, child, name, parse_xml, push_run, resolve, trim_runs};

/// A part's relationships: id → (resolved target, external).
type Rels = HashMap<String, (String, bool)>;

/// The relationships of `part` (`word/_rels/document.xml.rels`).
fn rels(pkg: &mut Package, part: &str) -> Rels {
    let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
    let rels_part = if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    };
    let Some(text) = pkg.xml(&rels_part) else {
        return Rels::new();
    };
    let Ok(doc) = parse_xml(&text) else {
        return Rels::new();
    };
    doc.root_element()
        .children()
        .filter(|n| name(*n) == "Relationship")
        .filter_map(|n| {
            let id = attr(n, "Id")?;
            let target = attr(n, "Target")?;
            let external = attr(n, "TargetMode") == Some("External");
            let target = if external {
                target.to_string()
            } else {
                resolve(part, target)
            };
            Some((id.to_string(), (target, external)))
        })
        .collect()
}

/// The part the package's `officeDocument` relationship points at.
fn main_part(pkg: &mut Package, fallback: &str) -> String {
    let Some(text) = pkg.xml("_rels/.rels") else {
        return fallback.to_string();
    };
    let Ok(doc) = parse_xml(&text) else {
        return fallback.to_string();
    };
    doc.root_element()
        .children()
        .filter(|n| name(*n) == "Relationship")
        .find(|n| attr(*n, "Type").is_some_and(|t| t.ends_with("/officeDocument")))
        .and_then(|n| attr(n, "Target"))
        .map(|t| t.trim_start_matches('/').to_string())
        .unwrap_or_else(|| fallback.to_string())
}

/// A relationship id attribute (`r:id`, `r:embed`), which can sit next
/// to a plain `id`.
fn rel_attr<'a>(node: Node<'a, '_>, local: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == local && a.namespace().is_some_and(|n| n.ends_with("/relationships")))
        .map(|a| a.value())
}

/// `w:b`, `a:rPr b="1"`: an on/off property (missing value = on).
fn on(node: Node, local: &str) -> bool {
    !matches!(attr(node, local), Some("0" | "false" | "off" | "none"))
}

// ---------------------------------------------------------------------------
// Word
// ---------------------------------------------------------------------------

/// What a paragraph style makes a paragraph.
#[derive(Clone, Copy, Default)]
struct ParaStyle {
    heading: Option<u8>,
    quote: bool,
    code: bool,
}

struct Word<'p, 'b> {
    pkg: Package<'b>,
    part: String,
    rels: Rels,
    styles: HashMap<String, ParaStyle>,
    /// numId → (ilvl → ordered)
    numbering: HashMap<String, HashMap<u8, bool>>,
    /// (numId, ilvl) → last number
    counters: HashMap<(String, u8), u32>,
    out: &'p mut Builder,
}

pub(super) fn read_word(bytes: &[u8]) -> Result<corvene_models::DocBlob, String> {
    let mut pkg = Package::open(bytes)?;
    let part = main_part(&mut pkg, "word/document.xml");
    let text = pkg.xml(&part).ok_or("no document part")?;
    let doc = parse_xml(&text)?;
    let doc_rels = rels(&mut pkg, &part);
    let dir = part.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    let styles = doc_rels
        .values()
        .find(|(t, ext)| !ext && t.ends_with("styles.xml"))
        .map(|(t, _)| t.clone())
        .unwrap_or_else(|| format!("{dir}/styles.xml"));
    let styles = word_styles(&mut pkg, &styles);
    let numbering = word_numbering(&mut pkg, &format!("{dir}/numbering.xml"));
    let mut out = Builder::new(DocKind::Text);
    let mut word = Word {
        pkg,
        part: part.clone(),
        rels: doc_rels,
        styles,
        numbering,
        counters: HashMap::new(),
        out: &mut out,
    };
    let body = doc
        .root_element()
        .children()
        .find(|n| name(*n) == "body")
        .ok_or("no body")?;
    word.blocks(body);
    // footnotes, after the body
    if let Some(text) = word.pkg.xml(&format!("{dir}/footnotes.xml"))
        && let Ok(notes) = parse_xml(&text)
    {
        word.part = format!("{dir}/footnotes.xml");
        word.rels = rels(&mut word.pkg, &word.part);
        for note in notes
            .root_element()
            .children()
            .filter(|n| name(*n) == "footnote" && attr(*n, "type").is_none())
        {
            let id = attr(note, "id").unwrap_or("");
            let mut first = true;
            for p in note.children().filter(|n| name(*n) == "p") {
                let (mut runs, images) = word.runs_of(p);
                if first {
                    runs.insert(0, DocRun::plain(format!("[{id}] ")));
                    first = false;
                }
                word.out.push(DocBlock::Paragraph {
                    runs: trim_runs(runs),
                    list: None,
                    quote: false,
                });
                word.push_images(images);
            }
        }
    }
    Ok(out.finish())
}

fn word_styles(pkg: &mut Package, part: &str) -> HashMap<String, ParaStyle> {
    let mut out = HashMap::new();
    let Some(text) = pkg.xml(part) else {
        return out;
    };
    let Ok(doc) = parse_xml(&text) else {
        return out;
    };
    let mut based_on = HashMap::new();
    for style in doc
        .root_element()
        .children()
        .filter(|n| name(*n) == "style")
    {
        if attr(style, "type") != Some("paragraph") {
            continue;
        }
        let Some(id) = attr(style, "styleId") else {
            continue;
        };
        let display = child(style, "name")
            .and_then(|n| attr(n, "val"))
            .unwrap_or(id)
            .to_ascii_lowercase();
        let outline = child(style, "pPr")
            .and_then(|p| child(p, "outlineLvl"))
            .and_then(|o| attr(o, "val"))
            .and_then(|v| v.parse::<u8>().ok());
        let heading = if display == "title" {
            Some(1)
        } else if display == "subtitle" {
            Some(2)
        } else if let Some(n) = display
            .strip_prefix("heading ")
            .and_then(|n| n.trim().parse::<u8>().ok())
        {
            Some(n.clamp(1, 6))
        } else {
            outline.filter(|l| *l < 9).map(|l| (l + 1).min(6))
        };
        let quote = display.contains("quote");
        let code = display.contains("code") || display.contains("preformatted");
        if let Some(base) = child(style, "basedOn").and_then(|b| attr(b, "val")) {
            based_on.insert(id.to_string(), base.to_string());
        }
        out.insert(
            id.to_string(),
            ParaStyle {
                heading,
                quote,
                code,
            },
        );
    }
    // a style based on a heading is a heading
    for (id, base) in &based_on {
        let mut seen = 0;
        let mut cur = base.clone();
        while seen < 16 {
            let Some(b) = out.get(&cur).copied() else {
                break;
            };
            if let Some(s) = out.get_mut(id) {
                if s.heading.is_none() {
                    s.heading = b.heading;
                }
                s.quote |= b.quote;
            }
            match based_on.get(&cur) {
                Some(next) => cur = next.clone(),
                None => break,
            }
            seen += 1;
        }
    }
    out
}

fn word_numbering(pkg: &mut Package, part: &str) -> HashMap<String, HashMap<u8, bool>> {
    let mut out = HashMap::new();
    let Some(text) = pkg.xml(part) else {
        return out;
    };
    let Ok(doc) = parse_xml(&text) else {
        return out;
    };
    let root = doc.root_element();
    let mut abstracts: HashMap<&str, HashMap<u8, bool>> = HashMap::new();
    for abs in root.children().filter(|n| name(*n) == "abstractNum") {
        let Some(id) = attr(abs, "abstractNumId") else {
            continue;
        };
        let levels = abs
            .children()
            .filter(|n| name(*n) == "lvl")
            .filter_map(|lvl| {
                let level = attr(lvl, "ilvl")?.parse::<u8>().ok()?;
                let fmt = child(lvl, "numFmt")
                    .and_then(|f| attr(f, "val"))
                    .unwrap_or("bullet");
                Some((level, !matches!(fmt, "bullet" | "none")))
            })
            .collect();
        abstracts.insert(id, levels);
    }
    for num in root.children().filter(|n| name(*n) == "num") {
        let (Some(id), Some(abs)) = (
            attr(num, "numId"),
            child(num, "abstractNumId").and_then(|a| attr(a, "val")),
        ) else {
            continue;
        };
        if let Some(levels) = abstracts.get(abs) {
            out.insert(id.to_string(), levels.clone());
        }
    }
    out
}

impl Word<'_, '_> {
    /// The block-level children of the body, a cell or a content control.
    fn blocks(&mut self, parent: Node) {
        for node in parent.children() {
            match name(node) {
                "p" => self.paragraph(node),
                "tbl" => {
                    let rows = self.table(node);
                    self.out.push(DocBlock::Table { rows });
                }
                "sdt" => {
                    if let Some(content) = child(node, "sdtContent") {
                        self.blocks(content);
                    }
                }
                "customXml" | "ins" | "smartTag" => self.blocks(node),
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, p: Node) {
        let ppr = child(p, "pPr");
        let style = ppr
            .and_then(|ppr| child(ppr, "pStyle"))
            .and_then(|s| attr(s, "val"))
            .and_then(|id| self.styles.get(id).copied())
            .unwrap_or_default();
        let outline = ppr
            .and_then(|ppr| child(ppr, "outlineLvl"))
            .and_then(|o| attr(o, "val"))
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|l| *l < 9);
        if ppr
            .and_then(|ppr| child(ppr, "pageBreakBefore"))
            .is_some_and(|b| on(b, "val"))
        {
            self.out.push(DocBlock::Break);
        }
        let list = ppr.and_then(|ppr| child(ppr, "numPr")).and_then(|num| {
            let id = child(num, "numId").and_then(|n| attr(n, "val"))?;
            if id == "0" {
                return None;
            }
            let level = child(num, "ilvl")
                .and_then(|l| attr(l, "val"))
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(0)
                .min(8);
            Some(self.list_mark(id, level))
        });
        let (runs, images) = self.runs_of(p);
        let runs = trim_runs(runs);
        let page_break = p
            .descendants()
            .any(|n| name(n) == "br" && attr(n, "type") == Some("page"));
        let heading = style.heading.or(outline.map(|l| (l + 1).min(6)));
        if style.code && list.is_none() && heading.is_none() {
            self.out.push(DocBlock::Code(DocBlock::runs_text(&runs)));
        } else if let (Some(level), None) = (heading, list) {
            self.out.push(DocBlock::Heading { level, runs });
        } else {
            self.out.push(DocBlock::Paragraph {
                runs,
                list,
                quote: style.quote,
            });
        }
        self.push_images(images);
        // text boxes: their paragraphs after this one
        let boxes: Vec<Node> = p
            .descendants()
            .filter(|n| name(*n) == "txbxContent" && !n.ancestors().any(|a| name(a) == "Fallback"))
            .collect();
        for content in boxes {
            self.blocks(content);
        }
        if page_break {
            self.out.push(DocBlock::Break);
        }
    }

    fn list_mark(&mut self, id: &str, level: u8) -> ListMark {
        let ordered = self
            .numbering
            .get(id)
            .and_then(|levels| levels.get(&level).copied())
            .unwrap_or(false);
        // deeper levels restart under a new item
        self.counters
            .retain(|(num, lvl), _| num != id || *lvl <= level);
        let n = self.counters.entry((id.to_string(), level)).or_insert(0);
        *n += 1;
        ListMark {
            level,
            ordered,
            number: *n,
        }
    }

    fn push_images(&mut self, images: Vec<(Option<String>, String)>) {
        for (target, alt) in images {
            let index = target.and_then(|t| {
                let bytes = self.pkg.bytes(&t);
                self.out.image(&t, bytes)
            });
            self.out.push(DocBlock::Image { index, alt });
        }
    }

    /// A paragraph's runs, and its pictures (target part, alt text).
    #[allow(clippy::type_complexity)]
    fn runs_of(&mut self, p: Node) -> (Vec<DocRun>, Vec<(Option<String>, String)>) {
        let mut runs = Vec::new();
        let mut images = Vec::new();
        self.inline(p, None, &mut runs, &mut images);
        (runs, images)
    }

    fn link_target(&self, node: Node) -> Option<String> {
        if let Some(id) = rel_attr(node, "id")
            && let Some((target, _)) = self.rels.get(id)
        {
            return Some(target.clone());
        }
        attr(node, "anchor").map(|a| format!("#{a}"))
    }

    fn inline(
        &mut self,
        parent: Node,
        link: Option<&str>,
        runs: &mut Vec<DocRun>,
        images: &mut Vec<(Option<String>, String)>,
    ) {
        for node in parent.children() {
            match name(node) {
                "r" => self.run(node, link, runs, images),
                "hyperlink" => {
                    let target = self.link_target(node);
                    self.inline(node, target.as_deref().or(link), runs, images);
                }
                "ins" | "smartTag" | "customXml" | "fldSimple" | "bdo" | "dir" => {
                    self.inline(node, link, runs, images)
                }
                "sdt" => {
                    if let Some(content) = child(node, "sdtContent") {
                        self.inline(content, link, runs, images);
                    }
                }
                // tracked deletions, moved-from text, comments
                _ => {}
            }
        }
    }

    fn run(
        &mut self,
        r: Node,
        link: Option<&str>,
        runs: &mut Vec<DocRun>,
        images: &mut Vec<(Option<String>, String)>,
    ) {
        let style = child(r, "rPr").map(run_style).unwrap_or_default();
        for node in r.children() {
            match name(node) {
                "t" => push_run(runs, node.text().unwrap_or(""), style, link),
                "tab" | "ptab" => push_run(runs, "\t", style, link),
                "br" if attr(node, "type") != Some("page") => push_run(runs, "\n", style, link),
                "cr" => push_run(runs, "\n", style, link),
                "noBreakHyphen" => push_run(runs, "\u{2011}", style, link),
                "sym" => {
                    if let Some(c) = attr(node, "char")
                        .and_then(|c| u32::from_str_radix(c, 16).ok())
                        .and_then(char::from_u32)
                    {
                        push_run(runs, &c.to_string(), style, link);
                    }
                }
                "drawing" | "pict" | "object" | "AlternateContent" => {
                    self.drawing(node, images);
                }
                _ => {}
            }
        }
    }

    /// The pictures in a drawing (its text boxes are read by
    /// [`Self::paragraph`]).
    fn drawing(&mut self, node: Node, images: &mut Vec<(Option<String>, String)>) {
        // `mc:AlternateContent`: only the first choice
        let node = if name(node) == "AlternateContent" {
            match node.children().find(|n| name(*n) == "Choice") {
                Some(choice) => choice,
                None => return,
            }
        } else {
            node
        };
        let alt = node
            .descendants()
            .find(|n| name(*n) == "docPr")
            .and_then(|d| attr(d, "descr").or_else(|| attr(d, "title")))
            .unwrap_or("")
            .to_string();
        for blip in node
            .descendants()
            .filter(|n| matches!(name(*n), "blip" | "imagedata"))
            .filter(|b| {
                !b.ancestors()
                    .take_while(|a| *a != node)
                    .any(|a| name(a) == "txbxContent")
            })
        {
            let id = rel_attr(blip, "embed").or_else(|| rel_attr(blip, "id"));
            let target = id
                .and_then(|id| self.rels.get(id))
                .and_then(|(t, ext)| (!ext).then(|| t.clone()));
            images.push((target, alt.clone()));
        }
    }

    fn table(&mut self, tbl: Node) -> Vec<Vec<Vec<DocRun>>> {
        let mut rows = Vec::new();
        for tr in tbl.children().filter(|n| name(*n) == "tr") {
            let mut row = Vec::new();
            for tc in tr.children().filter(|n| matches!(name(*n), "tc" | "sdt")) {
                let tc = if name(tc) == "sdt" {
                    match child(tc, "sdtContent").and_then(|c| child(c, "tc")) {
                        Some(tc) => tc,
                        None => continue,
                    }
                } else {
                    tc
                };
                row.push(self.cell(tc));
            }
            rows.push(row);
        }
        rows
    }

    /// A cell's paragraphs joined by `\n` (a nested table's cells too).
    fn cell(&mut self, tc: Node) -> Vec<DocRun> {
        let mut runs = Vec::new();
        for p in tc.descendants().filter(|n| name(*n) == "p") {
            let (r, _) = self.runs_of(p);
            let r = trim_runs(r);
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
}

/// `w:rPr` → formatting.
fn run_style(rpr: Node) -> RunStyle {
    let mut style = RunStyle::default();
    for node in rpr.children() {
        match name(node) {
            "b" => style.bold = on(node, "val"),
            "i" => style.italic = on(node, "val"),
            "u" => style.underline = on(node, "val"),
            "strike" | "dstrike" => style.strike = on(node, "val"),
            "vertAlign" => match attr(node, "val") {
                Some("superscript") => style.superscript = true,
                Some("subscript") => style.subscript = true,
                _ => {}
            },
            "rStyle" => {
                let id = attr(node, "val").unwrap_or("").to_ascii_lowercase();
                if id.contains("code") || id.contains("verbatim") {
                    style.code = true;
                }
                if id == "strong" {
                    style.bold = true;
                }
                if id == "emphasis" {
                    style.italic = true;
                }
            }
            _ => {}
        }
    }
    style
}

// ---------------------------------------------------------------------------
// PowerPoint
// ---------------------------------------------------------------------------

pub(super) fn read_presentation(bytes: &[u8]) -> Result<corvene_models::DocBlob, String> {
    let mut pkg = Package::open(bytes)?;
    let part = main_part(&mut pkg, "ppt/presentation.xml");
    let text = pkg.xml(&part).ok_or("no presentation part")?;
    let doc = parse_xml(&text)?;
    let rels = rels(&mut pkg, &part);
    let slides: Vec<String> = doc
        .descendants()
        .filter(|n| name(*n) == "sldId")
        .filter_map(|n| rel_attr(n, "id"))
        .filter_map(|id| rels.get(id).map(|(t, _)| t.clone()))
        .collect();
    let mut out = Builder::new(DocKind::Presentation);
    for slide in slides {
        read_slide(&mut pkg, &slide, &mut out);
    }
    Ok(out.finish())
}

fn read_slide(pkg: &mut Package, part: &str, out: &mut Builder) {
    let Some(text) = pkg.xml(part) else {
        out.section(String::new(), false);
        return;
    };
    let Ok(doc) = parse_xml(&text) else {
        out.section(String::new(), false);
        return;
    };
    let root = doc.root_element();
    out.section(String::new(), attr(root, "show") == Some("0"));
    let rels = rels(pkg, part);
    let mut slide = Slide {
        pkg,
        rels: &rels,
        out,
        notes: false,
    };
    if let Some(tree) = root.descendants().find(|n| name(*n) == "spTree") {
        // the title first, then the rest in drawing order
        let shapes: Vec<Node> = tree.children().filter(|n| n.is_element()).collect();
        let (titles, rest): (Vec<Node>, Vec<Node>) = shapes
            .into_iter()
            .partition(|s| matches!(placeholder(*s), Some("title" | "ctrTitle")));
        for shape in titles.into_iter().chain(rest) {
            slide.shape(shape);
        }
    }
    // speaker notes
    let notes = rels
        .values()
        .find(|(t, ext)| !ext && t.contains("notesSlide"))
        .map(|(t, _)| t.clone());
    if let Some(notes_part) = notes
        && let Some(text) = slide.pkg.xml(&notes_part)
        && let Ok(doc) = parse_xml(&text)
    {
        slide.notes = true;
        for shape in doc
            .descendants()
            .filter(|n| name(*n) == "sp" && placeholder(*n) == Some("body"))
        {
            slide.shape(shape);
        }
    }
}

/// `p:nvSpPr/p:nvPr/p:ph@type` (`Some("")` for an untyped placeholder,
/// which is a body).
fn placeholder<'a>(shape: Node<'a, '_>) -> Option<&'a str> {
    let nv = shape.children().find(|n| name(*n).starts_with("nv"))?;
    let ph = child(child(nv, "nvPr")?, "ph")?;
    Some(attr(ph, "type").unwrap_or("body"))
}

struct Slide<'a, 'b, 'r> {
    pkg: &'a mut Package<'b>,
    rels: &'r Rels,
    out: &'a mut Builder,
    notes: bool,
}

impl Slide<'_, '_, '_> {
    fn push(&mut self, block: DocBlock) {
        if self.notes {
            self.out.push_note(block);
        } else {
            self.out.push(block);
        }
    }

    fn shape(&mut self, shape: Node) {
        match name(shape) {
            "sp" | "cxnSp" => {
                let ph = placeholder(shape);
                let Some(body) = child(shape, "txBody") else {
                    return;
                };
                match ph {
                    Some("title" | "ctrTitle") => {
                        let runs = self.text_body_runs(body);
                        if !self.notes {
                            self.out
                                .name_section(&DocBlock::runs_text(&runs).replace('\n', " "));
                        }
                        self.push(DocBlock::Heading { level: 1, runs });
                    }
                    Some("subTitle") => {
                        let runs = self.text_body_runs(body);
                        self.push(DocBlock::Heading { level: 2, runs });
                    }
                    Some("sldNum" | "dt" | "ftr" | "hdr" | "sldImg") => {}
                    // notes are not bulleted
                    Some(_) => self.paragraphs(body, !self.notes),
                    None => self.paragraphs(body, false),
                }
            }
            "grpSp" => {
                for s in shape.children().filter(|n| n.is_element()) {
                    self.shape(s);
                }
            }
            "graphicFrame" => {
                if let Some(tbl) = shape.descendants().find(|n| name(*n) == "tbl") {
                    let rows = tbl
                        .children()
                        .filter(|n| name(*n) == "tr")
                        .map(|tr| {
                            tr.children()
                                .filter(|n| name(*n) == "tc")
                                .map(|tc| {
                                    child(tc, "txBody")
                                        .map(|b| self.text_body_runs(b))
                                        .unwrap_or_default()
                                })
                                .collect()
                        })
                        .collect();
                    self.push(DocBlock::Table { rows });
                }
            }
            "pic" => {
                let alt = shape
                    .descendants()
                    .find(|n| name(*n) == "cNvPr")
                    .and_then(|n| attr(n, "descr").or_else(|| attr(n, "name")))
                    .unwrap_or("")
                    .to_string();
                let target = shape
                    .descendants()
                    .find(|n| name(*n) == "blip")
                    .and_then(|b| rel_attr(b, "embed"))
                    .and_then(|id| self.rels.get(id))
                    .and_then(|(t, ext)| (!ext).then(|| t.clone()));
                let index = target.and_then(|t| {
                    let bytes = self.pkg.bytes(&t);
                    self.out.image(&t, bytes)
                });
                self.push(DocBlock::Image { index, alt });
            }
            "AlternateContent" => {
                if let Some(choice) = shape.children().find(|n| name(*n) == "Choice") {
                    for s in choice.children().filter(|n| n.is_element()) {
                        self.shape(s);
                    }
                }
            }
            _ => {}
        }
    }

    /// A text body's paragraphs joined by `\n` (titles, table cells).
    fn text_body_runs(&self, body: Node) -> Vec<DocRun> {
        let mut runs = Vec::new();
        for p in body.children().filter(|n| name(*n) == "p") {
            let r = trim_runs(self.runs(p));
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

    /// The paragraphs of a text box; `bullets` for a body placeholder,
    /// whose paragraphs are bulleted unless they say otherwise.
    fn paragraphs(&mut self, body: Node, bullets: bool) {
        let mut counters = [0u32; 9];
        for p in body.children().filter(|n| name(*n) == "p") {
            let ppr = child(p, "pPr");
            let level = ppr
                .and_then(|ppr| attr(ppr, "lvl"))
                .and_then(|l| l.parse::<u8>().ok())
                .unwrap_or(0)
                .min(8);
            let ordered = ppr.and_then(|ppr| child(ppr, "buAutoNum")).is_some();
            let none = ppr.and_then(|ppr| child(ppr, "buNone")).is_some();
            let bullet = ppr.and_then(|ppr| child(ppr, "buChar")).is_some();
            let runs = trim_runs(self.runs(p));
            if runs.is_empty() {
                continue;
            }
            let list = if ordered || (!none && (bullet || bullets)) {
                for c in counters.iter_mut().skip(usize::from(level) + 1) {
                    *c = 0;
                }
                counters[usize::from(level)] += 1;
                Some(ListMark {
                    level,
                    ordered,
                    number: counters[usize::from(level)],
                })
            } else {
                None
            };
            self.push(DocBlock::Paragraph {
                runs,
                list,
                quote: false,
            });
        }
    }

    fn runs(&self, p: Node) -> Vec<DocRun> {
        let mut runs = Vec::new();
        for node in p.children() {
            match name(node) {
                "r" | "fld" => {
                    let rpr = child(node, "rPr");
                    let style = rpr.map(drawing_style).unwrap_or_default();
                    let link = rpr
                        .and_then(|rpr| child(rpr, "hlinkClick"))
                        .and_then(|h| rel_attr(h, "id"))
                        .and_then(|id| self.rels.get(id))
                        .map(|(t, _)| t.clone());
                    let text = child(node, "t").and_then(|t| t.text()).unwrap_or("");
                    push_run(&mut runs, text, style, link.as_deref());
                }
                "br" => push_run(&mut runs, "\n", RunStyle::default(), None),
                _ => {}
            }
        }
        runs
    }
}

/// `a:rPr` → formatting.
fn drawing_style(rpr: Node) -> RunStyle {
    let baseline = attr(rpr, "baseline")
        .and_then(|b| b.parse::<i32>().ok())
        .unwrap_or(0);
    RunStyle {
        bold: attr(rpr, "b").is_some_and(|_| on(rpr, "b")),
        italic: attr(rpr, "i").is_some_and(|_| on(rpr, "i")),
        underline: attr(rpr, "u").is_some_and(|u| u != "none"),
        strike: attr(rpr, "strike").is_some_and(|s| s != "noStrike"),
        superscript: baseline > 0,
        subscript: baseline < 0,
        code: false,
    }
}
