use corvene_models::{DocBlob, DocBlock, DocKind, DocPage, DocRun, DocSection, ListMark};
use hayro::hayro_syntax::Pdf;

use super::*;

fn blob(kind: DocKind, sections: Vec<Vec<DocBlock>>, page: Option<DocPage>) -> DocBlob {
    DocBlob {
        kind,
        sections: sections
            .into_iter()
            .map(|blocks| DocSection {
                blocks,
                ..DocSection::default()
            })
            .collect(),
        images: Vec::new(),
        truncated: false,
        page,
        file: Default::default(),
    }
}

fn para(text: &str) -> DocBlock {
    DocBlock::Paragraph {
        runs: vec![DocRun::plain(text)],
        list: None,
        quote: false,
    }
}

fn pages_of(pdf: Vec<u8>) -> Vec<(f32, f32)> {
    let pdf = Pdf::new(pdf).expect("a PDF");
    pdf.pages().iter().map(|p| p.render_dimensions()).collect()
}

fn has_fonts() -> bool {
    fonts::face(fonts::Family::Sans, false, false).is_some()
}

#[test]
fn long_text_flows_onto_pages() {
    if !has_fonts() {
        return;
    }
    let mut blocks = vec![DocBlock::Heading {
        level: 1,
        runs: vec![DocRun::plain("Title")],
    }];
    for i in 0..200 {
        blocks.push(para(&format!(
            "Paragraph {i} with some words that wrap across the line at least once or twice \
             when the column is the width of an A4 page."
        )));
    }
    blocks.push(DocBlock::Paragraph {
        runs: vec![DocRun::plain("item")],
        list: Some(ListMark {
            level: 1,
            ordered: true,
            number: 3,
        }),
        quote: false,
    });
    blocks.push(DocBlock::Table {
        rows: vec![
            vec![vec![DocRun::plain("a")], vec![DocRun::plain("b")]],
            vec![vec![DocRun::plain("1")], vec![DocRun::plain("2")]],
        ],
    });
    blocks.push(DocBlock::Break);
    blocks.push(para("After the break, with ünïcödé and 日本語."));
    let doc = blob(DocKind::Text, vec![blocks], None);
    let mut last = 0.0;
    let pdf = document_pdf(&doc, A4, &mut |f| last = f).expect("pdf");
    assert_eq!(last, 1.0);
    let pages = pages_of(pdf);
    assert!(pages.len() > 4, "{} pages", pages.len());
    assert!((pages[0].0 - 595.28).abs() < 0.5 && (pages[0].1 - 841.89).abs() < 0.5);
}

#[test]
fn file_page_size_and_slides() {
    if !has_fonts() {
        return;
    }
    let letter = DocPage {
        width: 61_200,
        height: 79_200,
        margins: [7200; 4],
    };
    let doc = blob(DocKind::Text, vec![vec![para("x")]], Some(letter));
    let pages = pages_of(document_pdf(&doc, A4, &mut |_| {}).expect("pdf"));
    assert_eq!(pages, vec![(612.0, 792.0)]);

    let slides = blob(
        DocKind::Presentation,
        vec![
            vec![DocBlock::Heading {
                level: 1,
                runs: vec![DocRun::plain("One")],
            }],
            vec![para("Two"); 40],
        ],
        Some(DocPage {
            width: 96_000,
            height: 54_000,
            margins: [0; 4],
        }),
    );
    let pages = pages_of(document_pdf(&slides, A4, &mut |_| {}).expect("pdf"));
    assert_eq!(pages, vec![(960.0, 540.0), (960.0, 540.0)]);
}

#[test]
fn empty_document_has_a_page() {
    let doc = blob(DocKind::Book, Vec::new(), None);
    let pages = pages_of(document_pdf(&doc, LETTER, &mut |_| {}).expect("pdf"));
    assert_eq!(pages.len(), 1);
}

#[test]
fn letter_countries() {
    assert_eq!(default_geometry(Some("US")).width, 612.0);
    assert_eq!(default_geometry(Some("de")).width, A4.width);
    assert_eq!(default_geometry(None).width, A4.width);
}
