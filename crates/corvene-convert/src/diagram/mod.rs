//! Corvene `1353-diagram-diff`: diagrams drawn as pictures, so a change
//! shows as a changed drawing (with the image diff's 2-up, swipe and onion
//! skin views) next to the source's line diff.
//!
//! - draw.io and Excalidraw files store every shape's geometry: they are
//!   drawn where they were placed ([`drawio`], [`excalidraw`]);
//! - Graphviz and Mermaid files describe structure only: they are laid out
//!   by Corvene's layered layout ([`layout`], [`graph`]) or the layouts of
//!   Mermaid's other diagram kinds ([`mermaid`]), so positions differ from
//!   what `dot` or Mermaid draw while nodes, edges and labels match.

pub mod dot;
pub mod drawio;
pub mod excalidraw;
pub mod graph;
pub mod layout;
pub mod mermaid;
pub mod svg;

use crate::Format;

/// The picture of one side.
pub fn to_svg(format: Format, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    match format {
        Format::DrawIo => drawio::render(text),
        Format::Excalidraw => excalidraw::render(text),
        Format::Dot => dot::parse(text).map(|g| graph::render(&g)),
        Format::Mermaid => mermaid::render(text),
        _ => Err("Not a diagram.".into()),
    }
}
