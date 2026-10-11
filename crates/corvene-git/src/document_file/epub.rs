//! EPUB 2 / 3 books: `META-INF/container.xml` → the package document → the
//! spine's XHTML chapters, a section each.

use std::collections::HashMap;

use corvene_models::{DocBlob, DocBlock, DocKind, DocRun, ListMark, RunStyle};
use roxmltree::Node;

use super::{
    Builder, Package, attr, child, collapse_ws, name, parse_xml, push_run, resolve, trim_runs,
};

/// Chapters read; the rest are left out.
const MAX_CHAPTERS: usize = 2_000;

pub(super) fn read(bytes: &[u8]) -> Result<DocBlob, String> {
    let mut pkg = Package::open(bytes)?;
    let container = pkg
        .xml("META-INF/container.xml")
        .ok_or("no container.xml")?;
    let container = parse_xml(&container)?;
    let opf_path = container
        .descendants()
        .find(|n| name(*n) == "rootfile")
        .and_then(|n| attr(n, "full-path"))
        .ok_or("no rootfile")?
        .to_string();
    let opf = pkg.xml(&opf_path).ok_or("no package document")?;
    let opf = parse_xml(&opf)?;
    let manifest: HashMap<&str, (&str, &str)> = opf
        .descendants()
        .filter(|n| name(*n) == "item")
        .filter_map(|n| {
            Some((
                attr(n, "id")?,
                (attr(n, "href")?, attr(n, "media-type").unwrap_or("")),
            ))
        })
        .collect();
    let chapters: Vec<String> = opf
        .descendants()
        .filter(|n| name(*n) == "itemref")
        .filter_map(|n| manifest.get(attr(n, "idref")?))
        .filter(|(_, media)| media.contains("html") || media.is_empty())
        .map(|(href, _)| resolve(&opf_path, href.split('#').next().unwrap_or(href)))
        .take(MAX_CHAPTERS)
        .collect();
    let mut out = Builder::new(DocKind::Book);
    for chapter in chapters {
        out.section(String::new(), false);
        let Some(text) = pkg.xml(&chapter) else {
            continue;
        };
        let text = prepare_xhtml(&text);
        let Ok(doc) = parse_xml(&text) else {
            out.push(DocBlock::Paragraph {
                runs: vec![DocRun::plain("(This chapter could not be read.)")],
                list: None,
                quote: false,
            });
            continue;
        };
        let root = doc.root_element();
        let title = root
            .descendants()
            .find(|n| name(*n) == "title")
            .and_then(|t| t.text())
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        let Some(body) = child(root, "body") else {
            continue;
        };
        let mut book = Book {
            pkg: &mut pkg,
            part: &chapter,
            out: &mut out,
        };
        book.flow(body, Ctx::default());
        if !title.is_empty() {
            out.name_section(&title);
        }
    }
    Ok(out.finish())
}

/// XHTML that parses without a DTD: the doctype dropped, HTML's named
/// entities as characters.
fn prepare_xhtml(text: &str) -> String {
    let mut text = text.to_string();
    if let Some(start) = text.find("<!DOCTYPE") {
        // an internal subset ends at "]>"
        let rest = &text[start..];
        let end = if let Some(bracket) = rest.find('[')
            && rest.find('>').is_some_and(|gt| bracket < gt)
        {
            rest.find("]>").map(|e| e + 2)
        } else {
            rest.find('>').map(|e| e + 1)
        };
        if let Some(end) = end {
            text.replace_range(start..start + end, "");
        }
    }
    if !text.contains('&') {
        return text;
    }
    const ENTITIES: &[(&str, &str)] = &[
        ("&nbsp;", "\u{a0}"),
        ("&mdash;", "—"),
        ("&ndash;", "–"),
        ("&hellip;", "…"),
        ("&lsquo;", "‘"),
        ("&rsquo;", "’"),
        ("&ldquo;", "“"),
        ("&rdquo;", "”"),
        ("&laquo;", "«"),
        ("&raquo;", "»"),
        ("&copy;", "©"),
        ("&reg;", "®"),
        ("&trade;", "™"),
        ("&shy;", "\u{ad}"),
        ("&bull;", "•"),
        ("&middot;", "·"),
        ("&times;", "×"),
        ("&deg;", "°"),
        ("&eacute;", "é"),
        ("&egrave;", "è"),
        ("&aacute;", "á"),
        ("&agrave;", "à"),
        ("&ouml;", "ö"),
        ("&uuml;", "ü"),
        ("&auml;", "ä"),
        ("&szlig;", "ß"),
        ("&thinsp;", "\u{2009}"),
        ("&ensp;", "\u{2002}"),
        ("&emsp;", "\u{2003}"),
        ("&zwj;", "\u{200d}"),
        ("&zwnj;", "\u{200c}"),
    ];
    for (entity, c) in ENTITIES {
        if text.contains(entity) {
            text = text.replace(entity, c);
        }
    }
    text
}

/// Inherited while walking the tree.
#[derive(Clone, Default)]
struct Ctx {
    style: RunStyle,
    link: Option<String>,
    quote: bool,
    list: Option<ListMark>,
}

struct Book<'p, 'b, 'o> {
    pkg: &'p mut Package<'b>,
    part: &'p str,
    out: &'o mut Builder,
}

/// Elements that start a block.
fn is_block(n: &str) -> bool {
    matches!(
        n,
        "p" | "div"
            | "section"
            | "article"
            | "aside"
            | "header"
            | "footer"
            | "main"
            | "nav"
            | "figure"
            | "figcaption"
            | "blockquote"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "pre"
            | "table"
            | "hr"
            | "address"
            | "body"
    )
}

impl Book<'_, '_, '_> {
    /// Flow content: inline nodes gather into anonymous paragraphs between
    /// the blocks.
    fn flow(&mut self, parent: Node, ctx: Ctx) {
        let mut runs = Vec::new();
        let mut after_space = true;
        let mut list = ctx.list;
        for node in parent.children() {
            let n = name(node);
            if node.is_element() && (is_block(n) || n == "img" || n == "svg") {
                self.flush(&mut runs, &ctx, &mut list);
                after_space = true;
                self.block(node, &ctx);
            } else {
                self.inline(
                    node,
                    &ctx.style,
                    ctx.link.as_deref(),
                    &mut runs,
                    &mut after_space,
                );
            }
        }
        self.flush(&mut runs, &ctx, &mut list);
    }

    fn flush(&mut self, runs: &mut Vec<DocRun>, ctx: &Ctx, list: &mut Option<ListMark>) {
        let r = trim_runs(std::mem::take(runs));
        if r.is_empty() {
            return;
        }
        self.out.push(DocBlock::Paragraph {
            runs: r,
            list: list.take(),
            quote: ctx.quote,
        });
    }

    fn block(&mut self, node: Node, ctx: &Ctx) {
        let n = name(node);
        match n {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = n[1..].parse::<u8>().unwrap_or(1);
                let runs = self.runs(node, &ctx.style);
                self.out
                    .name_section(&DocBlock::runs_text(&runs).replace('\n', " "));
                self.out.push(DocBlock::Heading { level, runs });
            }
            "ul" | "ol" => {
                let level = ctx.list.map_or(0, |l| l.level + 1).min(8);
                let start = attr(node, "start")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(1);
                for (number, li) in (start..).zip(node.children().filter(|c| name(*c) == "li")) {
                    let item = Ctx {
                        list: Some(ListMark {
                            level,
                            ordered: n == "ol",
                            number,
                        }),
                        ..ctx.clone()
                    };
                    self.flow(li, item);
                }
            }
            "blockquote" => self.flow(
                node,
                Ctx {
                    quote: true,
                    ..ctx.clone()
                },
            ),
            "pre" => {
                let text: String = node
                    .descendants()
                    .filter(|d| d.is_text())
                    .filter_map(|d| d.text())
                    .collect();
                self.out
                    .push(DocBlock::Code(text.trim_matches('\n').to_string()));
            }
            "table" => {
                let rows = node
                    .descendants()
                    .filter(|d| name(*d) == "tr")
                    .map(|tr| {
                        tr.children()
                            .filter(|c| matches!(name(*c), "td" | "th"))
                            .map(|cell| {
                                let style = RunStyle {
                                    bold: name(cell) == "th" || ctx.style.bold,
                                    ..ctx.style
                                };
                                self.runs(cell, &style)
                            })
                            .collect()
                    })
                    .collect();
                self.out.push(DocBlock::Table { rows });
            }
            "hr" => self.out.push(DocBlock::Break),
            "img" => {
                let alt = attr(node, "alt").unwrap_or("").to_string();
                self.image(attr(node, "src"), alt);
            }
            "svg" => {
                for image in node.descendants().filter(|d| name(*d) == "image") {
                    self.image(attr(image, "href"), String::new());
                }
            }
            "li" => self.flow(node, ctx.clone()),
            _ => self.flow(node, ctx.clone()),
        }
    }

    fn image(&mut self, src: Option<&str>, alt: String) {
        let index = src.filter(|s| !s.contains(':')).and_then(|src| {
            let path = resolve(self.part, src);
            let bytes = self.pkg.bytes(&path);
            self.out.image(&path, bytes)
        });
        self.out.push(DocBlock::Image { index, alt });
    }

    /// The text of a heading or a cell, line breaks kept.
    fn runs(&mut self, node: Node, style: &RunStyle) -> Vec<DocRun> {
        let mut runs = Vec::new();
        let mut after_space = true;
        for c in node.children() {
            if is_block(name(c)) {
                if !runs.is_empty() {
                    push_run(&mut runs, "\n", RunStyle::default(), None);
                    after_space = true;
                }
                let inner = self.runs(c, style);
                for run in inner {
                    push_run(&mut runs, &run.text, run.style, run.link.as_deref());
                }
            } else {
                self.inline(c, style, None, &mut runs, &mut after_space);
            }
        }
        trim_runs(runs)
    }

    fn inline(
        &mut self,
        node: Node,
        style: &RunStyle,
        link: Option<&str>,
        runs: &mut Vec<DocRun>,
        after_space: &mut bool,
    ) {
        if node.is_text() {
            let text = collapse_ws(node.text().unwrap_or(""), after_space);
            push_run(runs, &text, *style, link);
            return;
        }
        if !node.is_element() {
            return;
        }
        let mut style = *style;
        let mut link = link.map(str::to_string);
        match name(node) {
            "b" | "strong" => style.bold = true,
            "i" | "em" | "cite" | "dfn" | "var" => style.italic = true,
            "u" | "ins" => style.underline = true,
            "s" | "strike" | "del" => style.strike = true,
            "sup" => style.superscript = true,
            "sub" => style.subscript = true,
            "code" | "kbd" | "samp" | "tt" => style.code = true,
            "a" => {
                if let Some(href) = attr(node, "href") {
                    link = Some(href.to_string());
                }
            }
            "br" => {
                push_run(runs, "\n", style, link.as_deref());
                *after_space = true;
                return;
            }
            "script" | "style" | "head" => return,
            _ => {}
        }
        for c in node.children() {
            self.inline(c, &style, link.as_deref(), runs, after_space);
        }
    }
}
