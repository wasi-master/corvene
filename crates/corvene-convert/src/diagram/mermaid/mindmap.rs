//! Mermaid mind maps: the indented tree with its node shapes (`[square]`,
//! `(rounded)`, `((circle))`, `{{hexagon}}`, `))bang((`, `)cloud(`), laid
//! out as a tree growing right from the root.

use super::super::graph::{GNode, Shape};
use super::super::svg::{Anchor, Style, Svg, colors, text_width};
use super::label;

struct Item {
    node: GNode,
    children: Vec<usize>,
    depth: usize,
    parent: Option<usize>,
}

fn shape_and_text(raw: &str) -> (Shape, String) {
    let raw = raw.trim();
    // `id[text]` etc.: the id is dropped when a shape gives the text
    const SHAPES: [(&str, &str, u8); 7] = [
        ("((", "))", 1),
        ("))", "((", 2),
        ("{{", "}}", 3),
        ("[", "]", 4),
        ("(", ")", 5),
        (")", "(", 6),
        ("{", "}", 7),
    ];
    for (open, close, code) in SHAPES {
        if let Some(at) = raw.find(open)
            && raw.ends_with(close)
            && at + open.len() <= raw.len() - close.len()
        {
            let text = label(&raw[at + open.len()..raw.len() - close.len()]);
            let shape = match code {
                1 => Shape::Circle,
                2 => Shape::Hexagon,
                3 => Shape::Hexagon,
                4 => Shape::Rect,
                5 => Shape::Rounded,
                6 => Shape::Stadium,
                _ => Shape::Rounded,
            };
            return (shape, text);
        }
    }
    (Shape::Plain, label(raw))
}

pub fn render(lines: &[String], title: Option<String>) -> Result<Vec<u8>, String> {
    let mut items: Vec<Item> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (indent, item)
    let mut roots = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let text = line.trim();
        if text.starts_with("::icon(") || text.starts_with(":::") {
            continue;
        }
        let text = text.split(":::").next().unwrap_or(text);
        let (shape, label) = shape_and_text(text);
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let depth = stack.len();
        items.push(Item {
            node: GNode {
                label,
                shape,
                ..Default::default()
            },
            children: Vec::new(),
            depth,
            parent: stack.last().map(|(_, p)| *p),
        });
        let ix = items.len() - 1;
        match stack.last() {
            Some(&(_, parent)) => items[parent].children.push(ix),
            None => roots.push(ix),
        }
        stack.push((indent, ix));
        if items.len() > 3000 {
            break;
        }
    }
    if items.is_empty() {
        return Err("The mind map is empty.".into());
    }
    // tidy tree: leaves stack vertically, parents centered on children
    let size = |it: &Item| {
        let w = it
            .node
            .label
            .split('\n')
            .map(|l| text_width(l, 14.0))
            .fold(0.0, f64::max)
            + 28.0;
        let h = it.node.label.split('\n').count() as f64 * 18.0 + 14.0;
        match it.node.shape {
            Shape::Circle => {
                let d = w.max(h);
                (d, d)
            }
            _ => (w, h),
        }
    };
    let sizes: Vec<(f64, f64)> = items.iter().map(size).collect();
    let mut depth_x = Vec::new();
    for it in &items {
        if depth_x.len() <= it.depth {
            depth_x.resize(it.depth + 1, 0.0f64);
        }
    }
    let mut widest = vec![0.0f64; depth_x.len()];
    for (it, (w, _)) in items.iter().zip(&sizes) {
        widest[it.depth] = widest[it.depth].max(*w);
    }
    for d in 1..depth_x.len() {
        depth_x[d] = depth_x[d - 1] + widest[d - 1] / 2.0 + 60.0 + widest[d] / 2.0;
    }
    let mut ys = vec![0.0f64; items.len()];
    let mut next_y = 0.0f64;
    fn place(ix: usize, items: &[Item], sizes: &[(f64, f64)], ys: &mut [f64], next_y: &mut f64) {
        if items[ix].children.is_empty() {
            ys[ix] = *next_y + sizes[ix].1 / 2.0;
            *next_y += sizes[ix].1 + 14.0;
            return;
        }
        for &c in &items[ix].children {
            place(c, items, sizes, ys, next_y);
        }
        let first = ys[items[ix].children[0]];
        let last = ys[*items[ix].children.last().unwrap_or(&ix)];
        ys[ix] = (first + last) / 2.0;
    }
    for &r in &roots {
        place(r, &items, &sizes, &mut ys, &mut next_y);
        next_y += 20.0;
    }
    let mut svg = Svg::new();
    for (ix, it) in items.iter().enumerate() {
        for &c in &it.children {
            let (x0, y0) = (depth_x[it.depth] + sizes[ix].0 / 2.0, ys[ix]);
            let (x1, y1) = (depth_x[items[c].depth] - sizes[c].0 / 2.0, ys[c]);
            let mid = (x0 + x1) / 2.0;
            let d = format!(
                "M{},{} C{},{} {},{} {},{}",
                super::super::svg::n(x0),
                super::super::svg::n(y0),
                super::super::svg::n(mid),
                super::super::svg::n(y0),
                super::super::svg::n(mid),
                super::super::svg::n(y1),
                super::super::svg::n(x1),
                super::super::svg::n(y1)
            );
            let color = colors::SERIES[branch_of(&items, c) % colors::SERIES.len()];
            svg.path(&d, &Style::line(color, 2.0), None, None);
        }
    }
    for (ix, it) in items.iter().enumerate() {
        let (w, h) = sizes[ix];
        let (x, y) = (depth_x[it.depth], ys[ix]);
        let color = if it.depth == 0 {
            colors::NODE_STROKE
        } else {
            colors::SERIES[branch_of(&items, ix) % colors::SERIES.len()]
        };
        let fill = if it.depth == 0 {
            colors::NODE_FILL
        } else {
            "#ffffff"
        };
        let style = Style::new(fill, color, 1.6);
        match it.node.shape {
            Shape::Circle => svg.ellipse(x, y, w / 2.0, h / 2.0, &style),
            Shape::Rect => svg.rect(x - w / 2.0, y - h / 2.0, w, h, 0.0, &style),
            Shape::Rounded => svg.rect(x - w / 2.0, y - h / 2.0, w, h, 8.0, &style),
            Shape::Stadium => svg.rect(x - w / 2.0, y - h / 2.0, w, h, h / 2.0, &style),
            Shape::Hexagon => {
                let d = h / 2.0;
                let (l, r, t, b) = (x - w / 2.0, x + w / 2.0, y - h / 2.0, y + h / 2.0);
                svg.polygon(
                    &[
                        (l + d, t),
                        (r - d, t),
                        (r, y),
                        (r - d, b),
                        (l + d, b),
                        (l, y),
                    ],
                    &style,
                );
            }
            _ => {
                // a plain node is underlined in its branch color
                svg.line(
                    &[
                        (x - w / 2.0, y + h / 2.0 - 2.0),
                        (x + w / 2.0, y + h / 2.0 - 2.0),
                    ],
                    false,
                    &Style::line(color, 2.0),
                    None,
                    None,
                );
            }
        }
        svg.text(
            x,
            y,
            &it.node.label,
            14.0,
            Anchor::Middle,
            colors::INK,
            if it.depth == 0 { Some("600") } else { None },
        );
    }
    if let Some(t) = title {
        svg.text(
            0.0,
            -30.0,
            &t,
            18.0,
            Anchor::Start,
            colors::INK,
            Some("600"),
        );
    }
    Ok(svg.finish(16.0))
}

/// The index of the root's child a node descends from (its branch).
fn branch_of(items: &[Item], ix: usize) -> usize {
    let mut cur = ix;
    while let Some(p) = items[cur].parent {
        if items[p].depth == 0 {
            return items[p]
                .children
                .iter()
                .position(|c| *c == cur)
                .unwrap_or(0);
        }
        cur = p;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indented_tree() {
        let lines: Vec<String> = "  root((Ideas))\n    Origins\n      Long history\n      ::icon(fa fa-book)\n    Research[On effectiveness]\n    Tools\n      {{Pen}}"
            .lines()
            .map(str::to_string)
            .collect();
        let svg = String::from_utf8(render(&lines, None).unwrap()).unwrap();
        for t in [
            ">Ideas<",
            ">Origins<",
            ">Long history<",
            ">On effectiveness<",
            ">Pen<",
        ] {
            assert!(svg.contains(t), "{t}: {svg}");
        }
        assert!(svg.contains("<ellipse"));
        assert!(svg.contains("<polygon"));
    }
}
