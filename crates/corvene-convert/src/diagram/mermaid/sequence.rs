//! Mermaid sequence diagrams: participants and actors (with aliases, in
//! order of appearance), messages with their arrow kinds, self messages,
//! activations, notes, `loop`/`alt`/`else`/`opt`/`par`/`and`/`critical`/
//! `break`/`rect` blocks, `autonumber`, created and destroyed participants.

use super::super::svg::{Anchor, Style, Svg, colors, text_width};
use super::label;

const FONT: f64 = 14.0;
const ROW: f64 = 40.0;
const BOX_H: f64 = 38.0;

struct Participant {
    id: String,
    label: String,
    actor: bool,
}

enum Event {
    Message {
        from: usize,
        to: usize,
        text: String,
        dashed: bool,
        head: Option<&'static str>,
        number: Option<usize>,
    },
    Note {
        left: usize,
        right: usize,
        side: NoteSide,
        text: String,
    },
    Open {
        kind: String,
        text: String,
    },
    Else {
        text: String,
    },
    Close,
    Activate(usize, bool),
}

#[derive(Clone, Copy, PartialEq)]
enum NoteSide {
    Left,
    Right,
    Over,
}

struct Doc {
    participants: Vec<Participant>,
    events: Vec<Event>,
}

impl Doc {
    fn participant(&mut self, raw: &str) -> usize {
        let id = raw.trim().trim_matches(['+', '-']).trim();
        if let Some(ix) = self.participants.iter().position(|p| p.id == id) {
            return ix;
        }
        self.participants.push(Participant {
            id: id.to_string(),
            label: id.to_string(),
            actor: false,
        });
        self.participants.len() - 1
    }
}

/// The arrow at the start of `s` and its length.
fn arrow(s: &str) -> Option<(bool, Option<&'static str>, usize)> {
    const ARROWS: [(&str, bool, Option<&str>); 10] = [
        ("-->>", true, Some("arrow")),
        ("->>", false, Some("arrow")),
        ("<<-->>", true, Some("arrow")),
        ("<<->>", false, Some("arrow")),
        ("--x", true, Some("cross")),
        ("-x", false, Some("cross")),
        ("--)", true, Some("open")),
        ("-)", false, Some("open")),
        ("-->", true, None),
        ("->", false, None),
    ];
    ARROWS
        .iter()
        .find(|(a, _, _)| s.starts_with(a))
        .map(|(a, dashed, head)| (*dashed, *head, a.len()))
}

fn parse(lines: &[String]) -> Result<(Doc, Option<String>), String> {
    let mut doc = Doc {
        participants: Vec::new(),
        events: Vec::new(),
    };
    let mut title = None;
    let mut number: Option<usize> = None;
    let mut box_depth = 0;
    for line in lines {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        let word = s.split_whitespace().next().unwrap_or("");
        let rest = s[word.len()..].trim();
        match word {
            "participant" | "actor" | "create" | "destroy" => {
                let rest = rest
                    .trim_start_matches("participant ")
                    .trim_start_matches("actor ");
                let (id, alias) = match rest.split_once(" as ") {
                    Some((i, a)) => (i, Some(label(a))),
                    None => (rest, None),
                };
                if word == "destroy" {
                    continue;
                }
                let ix = doc.participant(id);
                if let Some(a) = alias {
                    doc.participants[ix].label = a;
                }
                if word == "actor" || s.starts_with("create actor") {
                    doc.participants[ix].actor = true;
                }
                continue;
            }
            "box" => {
                box_depth += 1;
                continue;
            }
            "title" | "title:" => {
                title = Some(rest.trim_start_matches(':').trim().to_string());
                continue;
            }
            "autonumber" => {
                number = Some(
                    rest.split_whitespace()
                        .next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(1),
                );
                continue;
            }
            "activate" | "deactivate" => {
                let ix = doc.participant(rest);
                doc.events.push(Event::Activate(ix, word == "activate"));
                continue;
            }
            "loop" | "alt" | "opt" | "par" | "critical" | "break" | "rect" => {
                doc.events.push(Event::Open {
                    kind: word.to_string(),
                    text: if word == "rect" {
                        String::new()
                    } else {
                        label(rest)
                    },
                });
                continue;
            }
            "else" | "and" | "option" => {
                doc.events.push(Event::Else { text: label(rest) });
                continue;
            }
            "end" => {
                if box_depth > 0 && !doc.events.iter().any(|e| matches!(e, Event::Open { .. })) {
                    box_depth -= 1;
                } else {
                    doc.events.push(Event::Close);
                }
                continue;
            }
            _ => {}
        }
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("note ") {
            let body = &s[5..];
            let (place, text) = body.split_once(':').unwrap_or((body, ""));
            let place = place.trim();
            let (side, who) = if let Some(w) = place.strip_prefix("right of") {
                (NoteSide::Right, w)
            } else if let Some(w) = place.strip_prefix("left of") {
                (NoteSide::Left, w)
            } else {
                (NoteSide::Over, place.trim_start_matches("over"))
            };
            let mut ids = who.split(',');
            let a = doc.participant(ids.next().unwrap_or(""));
            let b = ids.next().map(|w| doc.participant(w)).unwrap_or(a);
            doc.events.push(Event::Note {
                left: a.min(b),
                right: a.max(b),
                side,
                text: label(text),
            });
            continue;
        }
        // a message: `A->>+B: text`
        let Some((main, text)) = s.split_once(':').or(Some((s, ""))) else {
            continue;
        };
        let bytes = main.as_bytes();
        let mut found = None;
        for i in 0..bytes.len() {
            if matches!(bytes[i], b'-' | b'<')
                && let Some(a) = arrow(&main[i..])
            {
                found = Some((i, a));
                break;
            }
        }
        let Some((at, (dashed, head, len))) = found else {
            continue;
        };
        let from_raw = &main[..at];
        let to_raw = &main[at + len..];
        let from = doc.participant(from_raw);
        let to = doc.participant(to_raw);
        doc.events.push(Event::Message {
            from,
            to,
            text: label(text),
            dashed,
            head,
            number,
        });
        if let Some(n) = number.as_mut() {
            *n += 1;
        }
        let to_trim = to_raw.trim();
        if to_trim.starts_with('+') {
            doc.events.push(Event::Activate(to, true));
        } else if to_trim.starts_with('-') {
            doc.events.push(Event::Activate(from, false));
        }
    }
    if doc.participants.is_empty() {
        return Err("The sequence diagram has no participants.".into());
    }
    Ok((doc, title))
}

/// A block being drawn: kind, text, top, `else` lines (y, text).
type OpenFrame = (String, String, f64, Vec<(f64, String)>);
/// A drawn block: kind, text, top, bottom, `else` lines, nesting depth.
type Frame = (String, String, f64, f64, Vec<(f64, String)>, usize);

pub fn render(lines: &[String], title: Option<String>) -> Result<Vec<u8>, String> {
    // `+B`/`-B` targets carry their activation marks in the name
    let (doc, own_title) = parse(lines)?;
    let title = title.or(own_title);
    let n = doc.participants.len();
    // spacing: each gap fits the widest message between neighbors
    let widths: Vec<f64> = doc
        .participants
        .iter()
        .map(|p| (text_width(&p.label, FONT) + 30.0).max(90.0))
        .collect();
    let mut gaps = vec![0.0f64; n.saturating_sub(1)];
    for (i, g) in gaps.iter_mut().enumerate() {
        *g = (widths[i] + widths[i + 1]) / 2.0 + 40.0;
    }
    for e in &doc.events {
        match e {
            Event::Message { from, to, text, number, .. } if from != to => {
                let (a, b) = ((*from).min(*to), (*from).max(*to));
                let need = text.split('\n').map(|l| text_width(l, FONT - 1.0)).fold(0.0, f64::max)
                    + if number.is_some() { 30.0 } else { 0.0 }
                    + 30.0;
                let have: f64 = gaps[a..b].iter().sum();
                if need > have {
                    let extra = (need - have) / (b - a) as f64;
                    for g in &mut gaps[a..b] {
                        *g += extra;
                    }
                }
            }
            Event::Message { from, text, .. }
                // a self message's text sits right of its lifeline
                if *from < gaps.len() => {
                    let need = text_width(text, FONT - 1.0) + 60.0;
                    gaps[*from] = gaps[*from].max(need);
                }
            _ => {}
        }
    }
    let mut xs = vec![0.0; n];
    for i in 1..n {
        xs[i] = xs[i - 1] + gaps[i - 1];
    }
    let mut svg = Svg::new();
    let top = 0.0;
    let mut y = top + BOX_H + 30.0;
    let line = Style::line(colors::LINE, 1.4);
    let mut frames: Vec<OpenFrame> = Vec::new();
    let mut done_frames: Vec<Frame> = Vec::new();
    let mut activations: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut bars: Vec<(usize, f64, f64, usize)> = Vec::new();
    for e in &doc.events {
        match e {
            Event::Message {
                from,
                to,
                text,
                dashed,
                head,
                number,
            } => {
                let text = match number {
                    Some(k) => format!("{k}. {text}"),
                    None => text.clone(),
                };
                let lines_n = text.split('\n').count() as f64;
                y += (lines_n - 1.0) * 16.0;
                let mut style = line.clone();
                if *dashed {
                    style = style.dashed("5 4");
                }
                let off = |p: usize| activations[p].len() as f64 * 5.0;
                if from == to {
                    let x = xs[*from] + off(*from);
                    let pts = [
                        (x, y - 8.0),
                        (x + 40.0, y - 8.0),
                        (x + 40.0, y + 12.0),
                        (x + 4.0, y + 12.0),
                    ];
                    svg.line(&pts, false, &style, *head, None);
                    svg.text(
                        x + 46.0,
                        y + 2.0,
                        &text,
                        FONT - 1.0,
                        Anchor::Start,
                        colors::INK,
                        None,
                    );
                    y += 16.0;
                } else {
                    let dir = if to > from { 1.0 } else { -1.0 };
                    let x1 = xs[*from] + dir * off(*from).max(0.0);
                    let x2 = xs[*to] - dir * (off(*to) + 1.0);
                    svg.line(&[(x1, y), (x2, y)], false, &style, *head, None);
                    svg.text(
                        (x1 + x2) / 2.0,
                        y - 10.0 - (lines_n - 1.0) * 8.0,
                        &text,
                        FONT - 1.0,
                        Anchor::Middle,
                        colors::INK,
                        None,
                    );
                }
                y += ROW;
            }
            Event::Note {
                left,
                right,
                side,
                text,
            } => {
                let w = text
                    .split('\n')
                    .map(|l| text_width(l, FONT - 1.0))
                    .fold(0.0, f64::max)
                    + 20.0;
                let h = text.split('\n').count() as f64 * 17.0 + 12.0;
                let x = match side {
                    NoteSide::Right => xs[*left] + 12.0,
                    NoteSide::Left => xs[*left] - 12.0 - w,
                    NoteSide::Over => {
                        let (a, b) = (xs[*left], xs[*right]);
                        let span = (b - a + 60.0).max(w);
                        (a + b) / 2.0 - span / 2.0
                    }
                };
                let w = if *side == NoteSide::Over {
                    (xs[*right] - xs[*left] + 60.0).max(w)
                } else {
                    w
                };
                svg.rect(
                    x,
                    y - 14.0,
                    w,
                    h,
                    0.0,
                    &Style::new(colors::NOTE_FILL, colors::NOTE_STROKE, 1.0),
                );
                svg.text(
                    x + w / 2.0,
                    y - 14.0 + h / 2.0,
                    text,
                    FONT - 1.0,
                    Anchor::Middle,
                    colors::INK,
                    None,
                );
                y += h + 14.0;
            }
            Event::Open { kind, text } => {
                frames.push((kind.clone(), text.clone(), y - 26.0, Vec::new()));
                y += 22.0;
            }
            Event::Else { text } => {
                if let Some(f) = frames.last_mut() {
                    f.3.push((y - 16.0, text.clone()));
                }
                y += 22.0;
            }
            Event::Close => {
                if let Some((kind, text, start, elses)) = frames.pop() {
                    let depth = frames.len();
                    done_frames.push((kind, text, start, y - 12.0, elses, depth));
                    y += 12.0;
                }
            }
            Event::Activate(p, on) => {
                if *on {
                    activations[*p].push(y - ROW / 2.0);
                } else if let Some(start) = activations[*p].pop() {
                    let depth = activations[*p].len();
                    bars.push((*p, start, y - ROW / 2.0, depth));
                }
            }
        }
    }
    let bottom = y;
    for (p, starts) in activations.iter().enumerate() {
        for (depth, start) in starts.iter().enumerate() {
            bars.push((p, *start, bottom - 10.0, depth));
        }
    }
    // frames under everything, outer ones first
    done_frames.sort_by_key(|f| f.5);
    let left = xs[0] - widths[0] / 2.0 - 10.0;
    // frames hold self messages' and notes' text too
    let drawn_right = svg.bounds().map_or(f64::NEG_INFINITY, |b| b.2 + 10.0);
    let right = (xs[n - 1] + widths[n - 1] / 2.0 + 10.0).max(drawn_right);
    let mut frame_svg = Svg::new();
    for (kind, text, start, end, elses, depth) in &done_frames {
        let inset = *depth as f64 * 8.0;
        let (x0, x1) = (left + inset, right - inset);
        if kind == "rect" {
            frame_svg.rect(
                x0,
                *start,
                x1 - x0,
                end - start,
                0.0,
                &Style::new("#f6f8fa", "none", 0.0),
            );
            continue;
        }
        frame_svg.rect(
            x0,
            *start,
            x1 - x0,
            end - start,
            0.0,
            &Style::new("none", colors::MUTED, 1.0),
        );
        let tag_w = text_width(kind, FONT - 2.0) + 16.0;
        frame_svg.polygon(
            &[
                (x0, *start),
                (x0 + tag_w, *start),
                (x0 + tag_w, start + 14.0),
                (x0 + tag_w - 6.0, start + 20.0),
                (x0, start + 20.0),
            ],
            &Style::new("#eaeef2", colors::MUTED, 1.0),
        );
        frame_svg.text(
            x0 + 6.0,
            start + 10.0,
            kind,
            FONT - 2.0,
            Anchor::Start,
            colors::INK,
            Some("600"),
        );
        if !text.is_empty() {
            frame_svg.text(
                x0 + tag_w + 8.0,
                start + 10.0,
                &format!("[{text}]"),
                FONT - 2.0,
                Anchor::Start,
                colors::INK,
                None,
            );
        }
        for (ey, etext) in elses {
            frame_svg.line(
                &[(x0, *ey), (x1, *ey)],
                false,
                &Style::line(colors::MUTED, 1.0).dashed("4 3"),
                None,
                None,
            );
            if !etext.is_empty() {
                frame_svg.text(
                    x0 + 8.0,
                    ey + 10.0,
                    &format!("[{etext}]"),
                    FONT - 2.0,
                    Anchor::Start,
                    colors::INK,
                    None,
                );
            }
        }
    }
    let mut out = Svg::new();
    out.include(left, top);
    out.include(right, bottom + BOX_H + 10.0);
    // lifelines, boxes top and bottom
    for (i, p) in doc.participants.iter().enumerate() {
        let x = xs[i];
        out.line(
            &[(x, top + BOX_H), (x, bottom)],
            false,
            &Style::line(colors::CLUSTER_STROKE, 1.0).dashed("4 4"),
            None,
            None,
        );
        for by in [top, bottom] {
            participant_box(&mut out, x, by, widths[i], p);
        }
    }
    let merged = merge(out, frame_svg, svg, &bars, &xs);
    let mut merged = merged;
    if let Some(t) = title {
        merged.text(
            left,
            top - 24.0,
            &t,
            FONT + 4.0,
            Anchor::Start,
            colors::INK,
            Some("600"),
        );
    }
    Ok(merged.finish(16.0))
}

fn participant_box(svg: &mut Svg, x: f64, y: f64, w: f64, p: &Participant) {
    if p.actor {
        let ink = Style::line(colors::INK, 1.4);
        let cy = y + 6.0;
        svg.ellipse(x, cy, 5.0, 5.0, &Style::new("#ffffff", colors::INK, 1.4));
        svg.line(&[(x, cy + 5.0), (x, cy + 17.0)], false, &ink, None, None);
        svg.line(
            &[(x - 9.0, cy + 9.0), (x + 9.0, cy + 9.0)],
            false,
            &ink,
            None,
            None,
        );
        svg.line(
            &[(x - 8.0, cy + 26.0), (x, cy + 17.0), (x + 8.0, cy + 26.0)],
            false,
            &ink,
            None,
            None,
        );
        svg.text(
            x,
            y + BOX_H + 6.0,
            &p.label,
            FONT,
            Anchor::Middle,
            colors::INK,
            None,
        );
    } else {
        svg.rect(
            x - w / 2.0,
            y,
            w,
            BOX_H,
            4.0,
            &Style::new(colors::NODE_FILL, colors::NODE_STROKE, 1.3),
        );
        svg.text(
            x,
            y + BOX_H / 2.0,
            &p.label,
            FONT,
            Anchor::Middle,
            colors::INK,
            None,
        );
    }
}

/// Layers the parts: frames, lifelines and boxes, activation bars, then
/// messages and notes.
fn merge(
    base: Svg,
    frames: Svg,
    messages: Svg,
    bars: &[(usize, f64, f64, usize)],
    xs: &[f64],
) -> Svg {
    let mut out = Svg::new();
    out.absorb(frames);
    out.absorb(base);
    for (p, start, end, depth) in bars {
        let x = xs[*p] - 5.0 + *depth as f64 * 5.0;
        out.rect(
            x,
            *start,
            10.0,
            (end - start).max(8.0),
            0.0,
            &Style::new("#eaeef2", colors::MUTED, 1.0),
        );
    }
    out.absorb(messages);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_notes_and_blocks() {
        let lines: Vec<String> = "    autonumber\n    participant A as Alice\n    actor B as Bob\n    A->>+B: Hello Bob\n    B-->>-A: Hi\n    loop Every minute\n      A-)B: ping\n    end\n    alt ok\n      B->>B: think\n    else not ok\n      B--xA: fail\n    end\n    Note over A,B: shared"
            .lines()
            .map(str::to_string)
            .collect();
        let (doc, _) = parse(&lines).unwrap();
        assert_eq!(doc.participants.len(), 2);
        assert_eq!(doc.participants[0].label, "Alice");
        assert!(doc.participants[1].actor);
        let messages: Vec<&str> = doc
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Message { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(messages, vec!["Hello Bob", "Hi", "ping", "think", "fail"]);
        let svg = String::from_utf8(render(&lines, None).unwrap()).unwrap();
        assert!(svg.contains(">1. Hello Bob<"), "{svg}");
        assert!(svg.contains(">loop<"), "{svg}");
        assert!(svg.contains(">[not ok]<"), "{svg}");
        assert!(svg.contains(">shared<"), "{svg}");
    }
}
