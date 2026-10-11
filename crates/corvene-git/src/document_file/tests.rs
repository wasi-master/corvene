use std::io::Write;

use corvene_models::{DocBlock, DocKind, FileStatusKind, ListMark};

use super::*;

pub(crate) fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut out);
        for (name, bytes) in files {
            let o = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(*name, o).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    out.into_inner()
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000IHDR";

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#;

/// A .docx with `body` as its `w:body`.
pub(crate) fn docx(body: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}</w:body></w:document>"#
    );
    let styles = format!(
        r#"<w:styles {W}>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/></w:style>
<w:style w:type="paragraph" w:styleId="MyHead"><w:name w:val="My Head"/><w:basedOn w:val="Heading2"/></w:style>
<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/></w:style>
</w:styles>"#
    );
    let numbering = format!(
        r#"<w:numbering {W}>
<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
<w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"#
    );
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
</Relationships>"#;
    let root_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    zip_of(&[
        ("[Content_Types].xml", b"<Types/>"),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
        ("word/numbering.xml", numbering.as_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes()),
        ("word/media/image1.png", PNG),
    ])
}

fn p(style: &str, runs: &str) -> String {
    let ppr = if style.is_empty() {
        String::new()
    } else {
        format!(r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#)
    };
    format!("<w:p>{ppr}{runs}</w:p>")
}

fn r(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn blocks(blob: &corvene_models::DocBlob) -> &[DocBlock] {
    &blob.sections[0].blocks
}

#[test]
fn word_headings_styles_lists_and_tracked_changes() {
    let list = |num: u32, lvl: u32, text: &str| {
        format!(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{lvl}"/><w:numId w:val="{num}"/></w:numPr></w:pPr>{}</w:p>"#,
            r(text)
        )
    };
    let body = [
        p("Heading1", &r("Title")),
        p("MyHead", &r("Sub")),
        p(
            "",
            &format!(
                r#"{}<w:r><w:rPr><w:b/><w:i w:val="0"/></w:rPr><w:t>bold</w:t></w:r><w:ins>{}</w:ins><w:del><w:r><w:delText>gone</w:delText></w:r></w:del><w:hyperlink r:id="rId3">{}</w:hyperlink>"#,
                r("Plain "),
                r(" new"),
                r(" link")
            ),
        ),
        p("", ""),
        list(1, 0, "dot"),
        list(1, 1, "one"),
        list(1, 1, "two"),
        list(1, 0, "dot2"),
        list(1, 1, "again"),
        list(2, 0, "first"),
        p("Quote", &r("Said")),
    ]
    .concat();
    let blob = read(DocFormat::Word, &docx(&body)).unwrap();
    assert_eq!(blob.kind, DocKind::Text);
    let b = blocks(&blob);
    assert!(matches!(&b[0], DocBlock::Heading { level: 1, runs } if runs[0].text == "Title"));
    assert!(
        matches!(&b[1], DocBlock::Heading { level: 2, .. }),
        "{:?}",
        b[1]
    );
    let DocBlock::Paragraph { runs, .. } = &b[2] else {
        panic!("{:?}", b[2])
    };
    assert_eq!(DocBlock::runs_text(runs), "Plain bold new link");
    assert!(runs[1].style.bold && !runs[1].style.italic);
    assert_eq!(
        runs.last().unwrap().link.as_deref(),
        Some("https://example.com/")
    );
    // the empty paragraph is left out
    let marks: Vec<ListMark> = b[3..9]
        .iter()
        .map(|b| match b {
            DocBlock::Paragraph { list: Some(m), .. } => *m,
            other => panic!("{other:?}"),
        })
        .collect();
    let m = |level, ordered, number| ListMark {
        level,
        ordered,
        number,
    };
    assert_eq!(
        marks,
        [
            m(0, false, 1),
            m(1, true, 1),
            m(1, true, 2),
            m(0, false, 2),
            m(1, true, 1),
            m(0, true, 1)
        ]
    );
    assert!(matches!(&b[9], DocBlock::Paragraph { quote: true, .. }));
}

#[test]
fn word_table_image_and_page_break() {
    let body = [
        r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#.to_string(),
        p("", r#"<w:r><w:drawing><wp:inline><wp:docPr id="1" name="Picture 1" descr="A cat"/><a:graphic><a:graphicData><a:blip r:embed="rId2"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r><w:r><w:br w:type="page"/></w:r>"#),
        p("", &r("after")),
    ]
    .concat();
    let blob = read(DocFormat::Word, &docx(&body)).unwrap();
    let b = blocks(&blob);
    let DocBlock::Table { rows } = &b[0] else {
        panic!("{:?}", b[0])
    };
    assert_eq!(DocBlock::runs_text(&rows[0][0]), "a\nb");
    assert_eq!(DocBlock::runs_text(&rows[0][1]), "c");
    assert_eq!(
        b[1],
        DocBlock::Image {
            index: Some(0),
            alt: "A cat".into()
        }
    );
    assert_eq!(blob.images[0].media_type, "image/png");
    assert_eq!(b[2], DocBlock::Break);
    assert!(matches!(&b[3], DocBlock::Paragraph { .. }));
}

#[test]
fn word_text_lines() {
    let body = [p("Heading1", &r("Title")), p("", &r("Body"))].concat();
    let blob = read(DocFormat::Word, &docx(&body)).unwrap();
    assert_eq!(blob.to_text(), "# Title\n\nBody\n");
}

const P: &str = r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn sp(ph: Option<&str>, paras: &str) -> String {
    let ph = match ph {
        Some(t) => format!(r#"<p:ph type="{t}"/>"#),
        None => String::new(),
    };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="x"/><p:cNvSpPr/><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:txBody>{paras}</p:txBody></p:sp>"#
    )
}

fn ap(text: &str, ppr: &str) -> String {
    format!(r#"<a:p>{ppr}<a:r><a:rPr b="1"/><a:t>{text}</a:t></a:r></a:p>"#)
}

/// A .pptx with these slides (`(xml, hidden, notes)`).
pub(crate) fn pptx(slides: &[(&str, bool, Option<&str>)]) -> Vec<u8> {
    let ids: String = (0..slides.len())
        .map(|i| format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 256 + i, i + 10))
        .collect();
    let presentation =
        format!(r#"<p:presentation {P}><p:sldIdLst>{ids}</p:sldIdLst></p:presentation>"#);
    let rels: String = (0..slides.len())
        .map(|i| {
            format!(
                r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>"#,
                i + 10,
                i + 1
            )
        })
        .collect();
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
    );
    let root_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#;
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("_rels/.rels".into(), root_rels.into()),
        ("ppt/presentation.xml".into(), presentation.into_bytes()),
        ("ppt/_rels/presentation.xml.rels".into(), rels.into_bytes()),
        ("ppt/media/image1.png".into(), PNG.to_vec()),
    ];
    for (i, (tree, hidden, notes)) in slides.iter().enumerate() {
        let show = if *hidden { r#" show="0""# } else { "" };
        files.push((
            format!("ppt/slides/slide{}.xml", i + 1),
            format!(r#"<p:sld {P}{show}><p:cSld><p:spTree>{tree}</p:spTree></p:cSld></p:sld>"#)
                .into_bytes(),
        ));
        let mut rels = r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>"#.to_string();
        if let Some(notes) = notes {
            rels.push_str(&format!(
                r#"<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide{}.xml"/>"#,
                i + 1
            ));
            files.push((
                format!("ppt/notesSlides/notesSlide{}.xml", i + 1),
                format!(
                    r#"<p:notes {P}><p:cSld><p:spTree>{}{}</p:spTree></p:cSld></p:notes>"#,
                    sp(Some("sldImg"), ""),
                    sp(Some("body"), &ap(notes, ""))
                )
                .into_bytes(),
            ));
        }
        files.push((
            format!("ppt/slides/_rels/slide{}.xml.rels", i + 1),
            format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#).into_bytes(),
        ));
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    zip_of(&refs)
}

#[test]
fn presentation_slides_titles_bullets_notes_hidden() {
    let one = [
        sp(None, &ap("Free text", "")),
        sp(Some("title"), &ap("Hello", "")),
        sp(
            Some("body"),
            &[ap("Point", ""), ap("Deeper", r#"<a:pPr lvl="1"/>"#), ap("No bullet", "<a:pPr><a:buNone/></a:pPr>")].concat(),
        ),
        r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="Logo" descr="Logo"/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/></p:blipFill></p:pic>"#.to_string(),
    ]
    .concat();
    let two = sp(Some("ctrTitle"), &ap("Bye", ""));
    let blob = read(
        DocFormat::PowerPoint,
        &pptx(&[(&one, false, Some("Say hi")), (&two, true, None)]),
    )
    .unwrap();
    assert_eq!(blob.kind, DocKind::Presentation);
    assert_eq!(blob.sections.len(), 2);
    let s = &blob.sections[0];
    assert_eq!(s.name, "Hello");
    assert!(matches!(&s.blocks[0], DocBlock::Heading { level: 1, .. }));
    assert!(
        matches!(&s.blocks[1], DocBlock::Paragraph { list: None, runs, .. } if runs[0].text == "Free text" && runs[0].style.bold)
    );
    assert!(matches!(
        &s.blocks[2],
        DocBlock::Paragraph {
            list: Some(ListMark { level: 0, .. }),
            ..
        }
    ));
    assert!(matches!(
        &s.blocks[3],
        DocBlock::Paragraph {
            list: Some(ListMark { level: 1, .. }),
            ..
        }
    ));
    assert!(matches!(
        &s.blocks[4],
        DocBlock::Paragraph { list: None, .. }
    ));
    assert_eq!(
        s.blocks[5],
        DocBlock::Image {
            index: Some(0),
            alt: "Logo".into()
        }
    );
    assert_eq!(
        DocBlock::runs_text(match &s.notes[0] {
            DocBlock::Paragraph { runs, .. } => runs,
            other => panic!("{other:?}"),
        }),
        "Say hi"
    );
    assert!(blob.sections[1].hidden);
    assert_eq!(blob.sections[1].name, "Bye");
    let text = blob.to_text();
    assert!(
        text.starts_with("======== Slide 1: Hello ========\n# Hello\n"),
        "{text}"
    );
    assert!(text.contains("Notes:\n> Say hi\n"), "{text}");
    assert!(
        text.contains("======== Slide 2: Bye (hidden) ========"),
        "{text}"
    );
}

const ODF_NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0""#;

pub(crate) fn odf(body: &str) -> Vec<u8> {
    let content = format!(
        r#"<office:document-content {ODF_NS}><office:automatic-styles>
<style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
<style:style style:name="T2" style:family="text" style:parent-style-name="T1"><style:text-properties fo:font-style="italic"/></style:style>
<style:style style:name="dp2" style:family="drawing-page"><style:drawing-page-properties presentation:visibility="hidden"/></style:style>
<text:list-style style:name="L1"><text:list-level-style-number text:level="1"/><text:list-level-style-bullet text:level="2"/></text:list-style>
</office:automatic-styles><office:body>{body}</office:body></office:document-content>"#
    );
    zip_of(&[
        ("mimetype", b"application/vnd.oasis.opendocument.text"),
        ("content.xml", content.as_bytes()),
        ("Pictures/a.png", PNG),
    ])
}

#[test]
fn open_document_text() {
    let body = r#"<office:text>
<text:h text:outline-level="2">Part   one</text:h>
<text:p>Some <text:span text:style-name="T2">styled</text:span><text:s text:c="2"/>text <text:a xlink:href="https://x.org">here</text:a></text:p>
<text:p><draw:frame draw:name="Pic"><draw:image xlink:href="Pictures/a.png"/><svg:desc>Graph</svg:desc></draw:frame></text:p>
<text:list text:style-name="L1"><text:list-item><text:p>one</text:p><text:list><text:list-item><text:p>inner</text:p></text:list-item></text:list></text:list-item><text:list-item><text:p>two</text:p></text:list-item></text:list>
<table:table><table:table-row><table:table-cell><text:p>a</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1000"/></table:table-row><table:table-row table:number-rows-repeated="1000"><table:table-cell table:number-columns-repeated="1000"/></table:table-row></table:table>
</office:text>"#;
    let blob = read(DocFormat::OpenDocument, &odf(body)).unwrap();
    let b = blocks(&blob);
    assert!(
        matches!(&b[0], DocBlock::Heading { level: 2, runs } if DocBlock::runs_text(runs) == "Part one")
    );
    let DocBlock::Paragraph { runs, .. } = &b[1] else {
        panic!("{:?}", b[1])
    };
    assert_eq!(DocBlock::runs_text(runs), "Some styled  text here");
    assert!(runs[1].style.bold && runs[1].style.italic);
    assert_eq!(runs.last().unwrap().link.as_deref(), Some("https://x.org"));
    assert_eq!(
        b[2],
        DocBlock::Image {
            index: Some(0),
            alt: "Graph".into()
        }
    );
    let marks: Vec<Option<ListMark>> = b[3..6]
        .iter()
        .map(|b| match b {
            DocBlock::Paragraph { list, .. } => *list,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        marks,
        [
            Some(ListMark {
                level: 0,
                ordered: true,
                number: 1
            }),
            Some(ListMark {
                level: 1,
                ordered: false,
                number: 1
            }),
            Some(ListMark {
                level: 0,
                ordered: true,
                number: 2
            }),
        ]
    );
    let DocBlock::Table { rows } = &b[6] else {
        panic!("{:?}", b[6])
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 1);
}

#[test]
fn open_document_presentation() {
    let body = r#"<office:presentation>
<draw:page draw:name="page1"><draw:frame presentation:class="outline"><draw:text-box><text:list><text:list-item><text:p>Point</text:p></text:list-item></text:list></draw:text-box></draw:frame><draw:frame presentation:class="title"><draw:text-box><text:p>First</text:p></draw:text-box></draw:frame>
<presentation:notes><draw:frame presentation:class="notes"><draw:text-box><text:p>Remember</text:p></draw:text-box></draw:frame></presentation:notes></draw:page>
<draw:page draw:name="page2" draw:style-name="dp2"><draw:frame presentation:class="title"><draw:text-box><text:p>Second</text:p></draw:text-box></draw:frame></draw:page>
</office:presentation>"#;
    let blob = read(DocFormat::OpenDocument, &odf(body)).unwrap();
    assert_eq!(blob.kind, DocKind::Presentation);
    assert_eq!(blob.sections[0].name, "First");
    assert!(matches!(
        &blob.sections[0].blocks[0],
        DocBlock::Heading { level: 1, .. }
    ));
    assert!(matches!(
        &blob.sections[0].blocks[1],
        DocBlock::Paragraph { list: Some(_), .. }
    ));
    assert_eq!(blob.sections[0].notes.len(), 1);
    assert!(blob.sections[1].hidden);
}

#[test]
fn flat_open_document() {
    let fodt = format!(
        r#"<?xml version="1.0"?><office:document {ODF_NS}><office:body><office:text><text:p>Flat</text:p></office:text></office:body></office:document>"#
    );
    let blob = read(DocFormat::FlatOpenDocument, fodt.as_bytes()).unwrap();
    assert_eq!(blob.to_text(), "Flat\n");
}

pub(crate) fn epub(chapters: &[&str]) -> Vec<u8> {
    let container = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
    let items: String = (0..chapters.len())
        .map(|i| {
            format!(
                r#"<item id="c{i}" href="text/ch%20{i}.xhtml" media-type="application/xhtml+xml"/>"#
            )
        })
        .collect();
    // the spine lists the chapters in reverse to check its order wins
    let spine: String = (0..chapters.len())
        .rev()
        .map(|i| format!(r#"<itemref idref="c{i}"/>"#))
        .collect();
    let opf = format!(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><manifest>{items}<item id="img" href="img/a.png" media-type="image/png"/></manifest><spine>{spine}</spine></package>"#
    );
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("mimetype".into(), b"application/epub+zip".to_vec()),
        ("META-INF/container.xml".into(), container.into()),
        ("OEBPS/content.opf".into(), opf.into_bytes()),
        ("OEBPS/img/a.png".into(), PNG.to_vec()),
    ];
    for (i, body) in chapters.iter().enumerate() {
        files.push((
            format!("OEBPS/text/ch {i}.xhtml"),
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?><!DOCTYPE html><html xmlns="http://www.w3.org/1999/xhtml"><head><title>T{i}</title></head><body>{body}</body></html>"#
            )
            .into_bytes(),
        ));
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    zip_of(&refs)
}

#[test]
fn epub_chapters_in_spine_order() {
    let blob = read(
        DocFormat::Epub,
        &epub(&[
            "<p>Second&nbsp;chapter</p>",
            r#"<h1>Opening</h1><div>Loose <em>text</em><p>Para</p></div><ol start="3"><li>c</li><li>d<ul><li>x</li></ul></li></ol><blockquote><p>Q</p></blockquote><img src="../img/a.png" alt="A"/><pre>let x;
let y;</pre><table><tr><th>H</th><td>v</td></tr></table>"#,
        ]),
    )
    .unwrap();
    assert_eq!(blob.kind, DocKind::Book);
    assert_eq!(blob.sections.len(), 2);
    let s = &blob.sections[0];
    assert_eq!(s.name, "Opening");
    let text = blob.to_text();
    assert!(text.contains("# Opening\n\nLoose text\n\nPara\n\n3. c\n\n4. d\n\n  - x\n\n> Q\n\n[Image: A]\n\n```\nlet x;\nlet y;\n```\n\n| H | v |"), "{text}");
    assert_eq!(blob.images.len(), 1);
    assert_eq!(blob.sections[1].name, "T0");
    assert!(text.contains("Second\u{a0}chapter"), "{text}");
}

#[test]
fn rtf_formatting_headings_lists_tables() {
    let rtf = br#"{\rtf1\ansi\ansicpg1252{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}{\*\generator Test;}
{\pard\outlinelevel0 Heading\par}
\pard Plain {\b bold} {\i it\i0 al} caf\'e9 \u8364?, end\par
{\listtext 1.\tab}\pard\ls1\ilvl0 first\par
{\listtext 2.\tab}\pard\ls1\ilvl0 second\par
\pard\intbl a\cell b\cell\row
\pard after\par
}"#;
    let blob = read(DocFormat::Rtf, rtf).unwrap();
    let b = blocks(&blob);
    assert!(
        matches!(&b[0], DocBlock::Heading { level: 1, runs } if DocBlock::runs_text(runs) == "Heading"),
        "{:?}",
        b[0]
    );
    let DocBlock::Paragraph { runs, .. } = &b[1] else {
        panic!("{:?}", b[1])
    };
    assert_eq!(DocBlock::runs_text(runs), "Plain bold ital café €, end");
    assert!(runs.iter().any(|r| r.text == "bold" && r.style.bold));
    assert!(runs.iter().any(|r| r.text == "it" && r.style.italic));
    assert!(
        matches!(
            &b[2],
            DocBlock::Paragraph {
                list: Some(ListMark {
                    ordered: true,
                    number: 1,
                    ..
                }),
                ..
            }
        ),
        "{:?}",
        b[2]
    );
    assert!(matches!(
        &b[3],
        DocBlock::Paragraph {
            list: Some(ListMark { number: 2, .. }),
            ..
        }
    ));
    let DocBlock::Table { rows } = &b[4] else {
        panic!("{:?}", b[4])
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(DocBlock::runs_text(&rows[0][1]), "b");
    assert!(matches!(&b[5], DocBlock::Paragraph { .. }));
}

#[test]
fn rtf_png_picture() {
    let hex: String = PNG.iter().map(|b| format!("{b:02x}")).collect();
    let rtf = format!(
        r#"{{\rtf1 text\par{{\*\shppict{{\pict\pngblip\picw10\pich10 {hex}}}}}{{\nonshppict{{\pict\wmetafile8 0102}}}}\par}}"#
    );
    let blob = read(DocFormat::Rtf, rtf.as_bytes()).unwrap();
    let b = blocks(&blob);
    assert_eq!(b.len(), 2, "{b:?}");
    assert_eq!(
        b[1],
        DocBlock::Image {
            index: Some(0),
            alt: String::new()
        }
    );
    assert_eq!(blob.images[0].bytes, PNG);
}

#[test]
fn broken_files_do_not_read() {
    assert!(read(DocFormat::Word, b"not a zip").is_err());
    assert!(
        read(
            DocFormat::Word,
            &zip_of(&[("word/document.xml", b"<w:document")])
        )
        .is_err()
    );
    assert!(read(DocFormat::Rtf, b"plain").is_err());
    // a DTD (entity expansion) is refused
    let bomb = br#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY a "aaaa">]><w:document xmlns:w="w"><w:body/></w:document>"#;
    assert!(read(DocFormat::Word, &zip_of(&[("word/document.xml", bomb)])).is_err());
}

#[test]
fn oversized_part_is_not_read() {
    let big = vec![b' '; (MAX_PART + 1) as usize];
    let bytes = zip_of(&[("word/document.xml", &big)]);
    assert!(read(DocFormat::Word, &bytes).is_err());
}

#[test]
fn block_limit_truncates() {
    let body: String = (0..MAX_BLOCKS + 5)
        .map(|i| p("", &r(&i.to_string())))
        .collect();
    let blob = read(DocFormat::Word, &docx(&body)).unwrap();
    assert!(blob.truncated);
    assert_eq!(blob.block_count(), MAX_BLOCKS);
}

#[test]
fn diff_needs_the_flag_and_reading_sides() {
    let bytes = docx(&p("", &r("x")));
    set_read_document_files(false);
    assert!(
        document_diff(
            "a.docx",
            FileStatusKind::Modified,
            || Some(bytes.clone()),
            || Some(bytes.clone()),
            None
        )
        .is_none()
    );
    set_read_document_files(true);
    let diff = document_diff(
        "a.docx",
        FileStatusKind::New,
        || Some(bytes.clone()),
        || None,
        None,
    )
    .unwrap();
    assert!(matches!(
        diff,
        Diff::Document {
            previous: None,
            current: Some(_),
            source: None
        }
    ));
    // a side that does not read keeps the binary diff
    assert!(
        document_diff(
            "a.docx",
            FileStatusKind::Modified,
            || Some(bytes.clone()),
            || Some(b"junk".to_vec()),
            None
        )
        .is_none()
    );
    assert!(
        document_diff(
            "a.txt",
            FileStatusKind::New,
            || Some(bytes.clone()),
            || None,
            None
        )
        .is_none()
    );
}

#[test]
fn formats_from_extensions() {
    assert_eq!(document_format("a/B.DOCX"), Some(DocFormat::Word));
    assert_eq!(document_format("deck.ppsx"), Some(DocFormat::PowerPoint));
    assert_eq!(document_format("x.ott"), Some(DocFormat::OpenDocument));
    assert_eq!(document_format("x.fodp"), Some(DocFormat::FlatOpenDocument));
    assert_eq!(document_format("x.doc"), None);
    assert_eq!(
        resolve("ppt/slides/slide1.xml", "../media/a%20b.png"),
        "ppt/media/a b.png"
    );
    assert_eq!(
        resolve("word/document.xml", "/word/media/x.png"),
        "word/media/x.png"
    );
}
