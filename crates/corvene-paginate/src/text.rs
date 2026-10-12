//! A paragraph's runs shaped (rustybuzz, the shaper krilla embeds glyphs
//! for) with per-character font fallback, and broken into lines at the
//! Unicode line break opportunities (UAX #14), a word longer than the line
//! broken between its characters.

use std::ops::Range;
use std::sync::Arc;

use corvene_models::{DocRun, RunStyle};
use krilla::text::{GlyphId, KrillaGlyph};
use rustybuzz::ttf_parser;

use crate::fonts::{self, FaceRef, Family};

/// How a paragraph's text is set.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub size: f32,
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
    pub color: [u8; 3],
    /// Line height as a multiple of the size.
    pub leading: f32,
}

impl TextStyle {
    pub fn body(size: f32) -> Self {
        TextStyle {
            size,
            family: Family::Sans,
            bold: false,
            italic: false,
            color: [0x1f, 0x23, 0x28],
            leading: 1.3,
        }
    }
}

const LINK: [u8; 3] = [0x05, 0x63, 0xc1];

/// Glyphs of one face and formatting on a line, `x` from the line start.
#[derive(Clone)]
pub struct GlyphRun {
    pub face: FaceRef,
    pub size: f32,
    pub x: f32,
    pub width: f32,
    /// Raised (superscript) or lowered, in points.
    pub rise: f32,
    pub color: [u8; 3],
    pub underline: bool,
    pub strike: bool,
    pub glyphs: Vec<KrillaGlyph>,
    /// The paragraph's text, which the glyphs' ranges point into.
    pub text: Arc<str>,
}

/// One laid out line.
#[derive(Clone, Default)]
pub struct Line {
    pub width: f32,
    pub height: f32,
    /// From the top of the line box to the baseline.
    pub baseline: f32,
    pub runs: Vec<GlyphRun>,
}

/// A shaped stretch: one face, one formatting.
struct Segment {
    range: Range<usize>,
    face: FaceRef,
    size: f32,
    rise: f32,
    color: [u8; 3],
    underline: bool,
    strike: bool,
    /// Glyphs in visual order, advances per em.
    glyphs: Vec<KrillaGlyph>,
}

/// The runs of a paragraph laid out in lines at most `width` wide.
pub fn lay_out(runs: &[DocRun], style: TextStyle, width: f32) -> Vec<Line> {
    lay_out_inner(runs, style, width, true)
}

fn lay_out_inner(runs: &[DocRun], style: TextStyle, width: f32, break_words: bool) -> Vec<Line> {
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    let text: Arc<str> = Arc::from(text.as_str());
    let mut segments = Vec::new();
    let mut at = 0;
    for run in runs {
        let range = at..at + run.text.len();
        at = range.end;
        segment_run(
            &text,
            range,
            &run.style,
            run.link.is_some(),
            style,
            &mut segments,
        );
    }
    // advance of each cluster, keyed by its first byte
    let mut advance = vec![0f32; text.len() + 1];
    for seg in &segments {
        for g in &seg.glyphs {
            if g.text_range.start < advance.len() {
                advance[g.text_range.start] += g.x_advance * seg.size;
            }
        }
    }
    let mut prefix = vec![0f32; text.len() + 1];
    for i in 0..text.len() {
        prefix[i + 1] = prefix[i] + advance[i];
    }
    let span_width = |a: usize, b: usize| prefix[b.max(a)] - prefix[a];
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    let mut candidate: Option<usize> = None;
    let trim = |a: usize, b: usize| -> usize {
        let s = &text[a..b];
        a + s
            .trim_end_matches(|c: char| c.is_whitespace() && c != '\u{a0}')
            .len()
    };
    for (pos, opportunity) in unicode_linebreak::linebreaks(&text) {
        loop {
            let w = span_width(start, trim(start, pos));
            if w <= width {
                break;
            }
            if let Some(c) = candidate.take() {
                ranges.push(start..c);
                start = c;
                continue;
            }
            if !break_words {
                break;
            }
            // a word wider than the line: break between characters
            let end = trim(start, pos);
            let mut cut = start;
            for (i, ch) in text[start..end].char_indices() {
                let next = start + i + ch.len_utf8();
                if span_width(start, next) > width && cut > start {
                    break;
                }
                cut = next;
            }
            if cut >= end {
                break;
            }
            ranges.push(start..cut);
            start = cut;
        }
        if opportunity == unicode_linebreak::BreakOpportunity::Mandatory {
            ranges.push(start..pos);
            start = pos;
            candidate = None;
        } else {
            candidate = Some(pos);
        }
    }
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    ranges
        .into_iter()
        .map(|range| {
            let end = trim(range.start, range.end);
            line(&text, &segments, range.start..end, &prefix, style)
        })
        .collect()
}

/// The glyphs of `range` as a line.
fn line(
    text: &Arc<str>,
    segments: &[Segment],
    range: Range<usize>,
    prefix: &[f32],
    style: TextStyle,
) -> Line {
    let mut runs = Vec::new();
    let mut x = 0.0;
    let mut ascent = style.size * 0.9;
    let mut descent = style.size * 0.25;
    let mut tallest = style.size;
    for seg in segments {
        if seg.range.end <= range.start || seg.range.start >= range.end {
            continue;
        }
        let glyphs: Vec<KrillaGlyph> = seg
            .glyphs
            .iter()
            .filter(|g| range.contains(&g.text_range.start))
            .cloned()
            .collect();
        if glyphs.is_empty() {
            continue;
        }
        let width: f32 = glyphs.iter().map(|g| g.x_advance * seg.size).sum();
        ascent = ascent.max(seg.face.ascender * seg.size + seg.rise);
        descent = descent.max(-seg.face.descender * seg.size - seg.rise);
        tallest = tallest.max(seg.size);
        runs.push(GlyphRun {
            face: seg.face.clone(),
            size: seg.size,
            x,
            width,
            rise: seg.rise,
            color: seg.color,
            underline: seg.underline,
            strike: seg.strike,
            glyphs,
            text: text.clone(),
        });
        x += width;
    }
    let width = prefix[range.end] - prefix[range.start];
    let height = (tallest * style.leading).max(ascent + descent);
    Line {
        width,
        height,
        baseline: (height - (ascent + descent)) / 2.0 + ascent,
        runs,
    }
}

/// Split a run into stretches of one face (the style's face, or a fallback
/// face for characters it lacks) and shape them.
fn segment_run(
    text: &Arc<str>,
    range: Range<usize>,
    run: &RunStyle,
    link: bool,
    style: TextStyle,
    out: &mut Vec<Segment>,
) {
    let family = if run.code { Family::Mono } else { style.family };
    let bold = style.bold || run.bold;
    let italic = style.italic || run.italic;
    let Some(primary) = fonts::face(family, bold, italic) else {
        return;
    };
    let small = run.superscript || run.subscript;
    let size = if small { style.size * 0.65 } else { style.size };
    let rise = if run.superscript {
        style.size * 0.35
    } else if run.subscript {
        -style.size * 0.15
    } else {
        0.0
    };
    let color = if link { LINK } else { style.color };
    let parsed = ttf_parser::Face::parse(&primary.data, primary.index).ok();
    let mut current: Option<(usize, FaceRef)> = None;
    let flush = |from: usize, to: usize, face: &FaceRef, out: &mut Vec<Segment>| {
        if from >= to {
            return;
        }
        out.push(Segment {
            range: from..to,
            face: face.clone(),
            size,
            rise,
            color,
            underline: run.underline || link,
            strike: run.strike,
            glyphs: shape(text, from..to, face),
        });
    };
    for (i, c) in text[range.clone()].char_indices() {
        let at = range.start + i;
        if c == '\n' || c == '\r' {
            if let Some((from, face)) = current.take() {
                flush(from, at, &face, out);
            }
            continue;
        }
        let keep = c.is_whitespace() || fonts::joins_previous(c) || c.is_control();
        let face = if keep && current.is_some() {
            None
        } else if parsed.as_ref().is_none_or(|p| fonts::covers(p, c)) || keep {
            Some(primary.clone())
        } else {
            Some(fonts::fallback(c, bold, italic).unwrap_or_else(|| primary.clone()))
        };
        if let Some(face) = face {
            match &current {
                Some((_, cur)) if Arc::ptr_eq(cur, &face) => {}
                _ => {
                    if let Some((from, cur)) = current.take() {
                        flush(from, at, &cur, out);
                    }
                    current = Some((at, face));
                }
            }
        }
    }
    if let Some((from, face)) = current.take() {
        flush(from, range.end, &face, out);
    }
}

/// Shape `range` of `text` with one face; glyph ranges index `text`.
fn shape(text: &str, range: Range<usize>, face: &FaceRef) -> Vec<KrillaGlyph> {
    let Some(mut rb) = rustybuzz::Face::from_slice(&face.data, face.index) else {
        return Vec::new();
    };
    for (tag, value) in &face.variations {
        rb.set_variation(*tag, *value);
    }
    let upem = (rb.units_per_em() as f32).max(1.0);
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(&text[range.clone()]);
    buffer.guess_segment_properties();
    let output = rustybuzz::shape(&rb, &[], buffer);
    let infos = output.glyph_infos();
    let positions = output.glyph_positions();
    let len = range.end - range.start;
    // cluster starts in logical order give each glyph its end
    let mut starts: Vec<usize> = infos.iter().map(|i| i.cluster as usize).collect();
    starts.sort_unstable();
    starts.dedup();
    let end_of = |start: usize| -> usize {
        match starts.binary_search(&start) {
            Ok(i) => starts.get(i + 1).copied().unwrap_or(len),
            Err(_) => len,
        }
    };
    infos
        .iter()
        .zip(positions)
        .map(|(info, pos)| {
            let start = info.cluster as usize;
            KrillaGlyph::new(
                GlyphId::new(info.glyph_id),
                pos.x_advance as f32 / upem,
                pos.x_offset as f32 / upem,
                pos.y_offset as f32 / upem,
                pos.y_advance as f32 / upem,
                range.start + start..range.start + end_of(start),
                None,
            )
        })
        .collect()
}

/// The natural width of runs set on one line (no wrapping), and the width
/// of their longest word: a table column's widths.
pub fn measure(runs: &[DocRun], style: TextStyle) -> (f32, f32) {
    let lines = lay_out(runs, style, f32::INFINITY);
    let natural = lines.iter().map(|l| l.width).fold(0.0, f32::max);
    let words = lay_out_inner(runs, style, 0.0, false);
    let longest = words.iter().map(|l| l.width).fold(0.0, f32::max);
    (natural, longest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words() {
        if fonts::face(Family::Sans, false, false).is_none() {
            return;
        }
        let runs = vec![DocRun::plain(
            "one two three four five six seven eight nine ten",
        )];
        let style = TextStyle::body(11.0);
        let one = lay_out(&runs, style, 10_000.0);
        assert_eq!(one.len(), 1);
        let many = lay_out(&runs, style, 60.0);
        assert!(many.len() > 3, "{}", many.len());
        assert!(many.iter().all(|l| l.width <= 60.0 || l.runs.len() == 1));
        let breaks = lay_out(&[DocRun::plain("a\nb\n\nc")], style, 1000.0);
        assert_eq!(breaks.len(), 4);
    }
}
