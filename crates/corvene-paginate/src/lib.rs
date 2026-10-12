//! Corvene `1358-document-own-pages`: a document read for the document
//! diff (`corvene_models::DocBlob`) laid out on pages and written as a PDF
//! file, for the Pages view of a document diff when LibreOffice is not
//! installed or cannot convert the file.
//!
//! The pages follow the file's page or slide size and margins where it
//! sets them (Word `w:sectPr`, OpenDocument page layout, PowerPoint
//! `p:sldSz`, RTF `\paperw`), else A4 or US Letter by the locale. Text is
//! set in the platform's UI fonts (the faces the Rendered view uses),
//! shaped with rustybuzz with per-character font fallback, and embedded as
//! subset fonts by krilla, so the PDF has real, selectable text.
//!
//! This is not LibreOffice: document fonts, sizes, colours, alignment,
//! spacing, headers, footers and floating objects are not read, so page
//! breaks fall where Corvene's layout puts them.
//!
//! The crate reads bytes only; it never touches git, the network or the UI.

mod fonts;
mod layout;
mod render;
mod text;

use corvene_models::DocBlob;

pub use layout::Geometry;

/// ISO A4 with 2.5 cm margins, in points.
pub const A4: Geometry = Geometry {
    width: 595.28,
    height: 841.89,
    margins: [70.87, 70.87, 70.87, 70.87],
};

/// US Letter with 1 inch margins, in points.
pub const LETTER: Geometry = Geometry {
    width: 612.0,
    height: 792.0,
    margins: [72.0, 72.0, 72.0, 72.0],
};

/// Countries whose default paper is US Letter.
const LETTER_COUNTRIES: &[&str] = &[
    "US", "CA", "MX", "PH", "CL", "CO", "VE", "CR", "GT", "PA", "SV", "DO", "PR", "NI", "BZ",
];

/// The default page of a country (`US` → Letter, else A4).
pub fn default_geometry(country: Option<&str>) -> Geometry {
    match country {
        Some(c) if LETTER_COUNTRIES.contains(&c.to_ascii_uppercase().as_str()) => LETTER,
        _ => A4,
    }
}

/// The page geometry of a document: its own, else `fallback` (slides keep
/// no margins).
pub fn geometry(doc: &DocBlob, fallback: Geometry) -> Geometry {
    match doc.page {
        Some(page) => {
            let pt = |v: i32| v as f32 / 100.0;
            let width = pt(page.width).clamp(36.0, 14_400.0);
            let height = pt(page.height).clamp(36.0, 14_400.0);
            let mut margins = page.margins.map(|m| pt(m).max(0.0));
            // margins that leave no room are ignored
            if margins[1] + margins[3] > width * 0.8 || margins[0] + margins[2] > height * 0.8 {
                margins = [0.0; 4];
            }
            Geometry {
                width,
                height,
                margins,
            }
        }
        None => fallback,
    }
}

/// `doc` laid out on pages as a PDF file. `progress` gets the fraction
/// done (0 to 1) now and then. Takes a while for a long document; call it
/// off the UI thread.
pub fn document_pdf(
    doc: &DocBlob,
    fallback: Geometry,
    progress: &mut dyn FnMut(f32),
) -> Result<Vec<u8>, String> {
    let geometry = geometry(doc, fallback);
    let images = render::Images::new(&doc.images);
    let sizes = images.sizes();
    let pages = layout::pages(doc, geometry, &sizes, &mut |done, total| {
        if total > 0 {
            progress(0.9 * done as f32 / total as f32);
        }
    });
    let pdf = render::pdf(&pages, &images)?;
    progress(1.0);
    Ok(pdf)
}

#[cfg(test)]
mod tests;
