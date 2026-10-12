//! Mermaid pie charts: slices in file order with a legend (`showData`
//! adds the values).

use super::super::svg::{Anchor, Style, Svg, colors, n};
use super::label;

pub fn render(header: &str, lines: &[String], title: Option<String>) -> Result<Vec<u8>, String> {
    let mut show_data = header.contains("showData");
    let mut title = title.or_else(|| {
        header
            .split_once("title")
            .map(|(_, t)| t.trim().to_string())
            .filter(|t| !t.is_empty())
    });
    let mut slices: Vec<(String, f64)> = Vec::new();
    for line in lines {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        if s == "showData" {
            show_data = true;
            continue;
        }
        if let Some(t) = s.strip_prefix("title") {
            title = Some(t.trim().to_string());
            continue;
        }
        if let Some((name, value)) = s.rsplit_once(':')
            && let Ok(v) = value.trim().parse::<f64>()
            && v.is_finite()
            && v >= 0.0
        {
            slices.push((label(name.trim()), v));
        }
    }
    let total: f64 = slices.iter().map(|(_, v)| v).sum();
    if slices.is_empty() || total <= 0.0 {
        return Err("The pie chart has no slices.".into());
    }
    let mut svg = Svg::new();
    let (cx, cy, r) = (0.0, 0.0, 150.0);
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (ix, (name, v)) in slices.iter().enumerate() {
        let color = colors::SERIES[ix % colors::SERIES.len()];
        let sweep = v / total * std::f64::consts::TAU;
        let style = Style::new(color, "#ffffff", 2.0);
        if (sweep - std::f64::consts::TAU).abs() < 1e-9 {
            svg.ellipse(cx, cy, r, r, &style);
        } else if sweep > 0.0 {
            let (x0, y0) = (cx + r * angle.cos(), cy + r * angle.sin());
            let end = angle + sweep;
            let (x1, y1) = (cx + r * end.cos(), cy + r * end.sin());
            let large = if sweep > std::f64::consts::PI { 1 } else { 0 };
            let d = format!(
                "M{},{} L{},{} A{r},{r} 0 {large} 1 {},{} Z",
                n(cx),
                n(cy),
                n(x0),
                n(y0),
                n(x1),
                n(y1),
                r = n(r)
            );
            svg.include_rect(cx - r, cy - r, 2.0 * r, 2.0 * r);
            svg.path(&d, &style, None, None);
            let mid = angle + sweep / 2.0;
            let pct = v / total * 100.0;
            if pct >= 4.0 {
                svg.text(
                    cx + r * 0.65 * mid.cos(),
                    cy + r * 0.65 * mid.sin(),
                    &format!("{pct:.0}%"),
                    13.0,
                    Anchor::Middle,
                    "#ffffff",
                    Some("600"),
                );
            }
        }
        angle += sweep;
        // legend
        let ly = -r + ix as f64 * 24.0 + 8.0;
        svg.rect(
            r + 40.0,
            ly - 8.0,
            16.0,
            16.0,
            3.0,
            &Style::new(color, "none", 0.0),
        );
        let text = if show_data {
            format!("{name} [{}]", trim_number(*v))
        } else {
            name.clone()
        };
        svg.text(r + 64.0, ly, &text, 14.0, Anchor::Start, colors::INK, None);
    }
    if let Some(t) = title {
        svg.text(
            cx,
            cy - r - 30.0,
            &t,
            18.0,
            Anchor::Middle,
            colors::INK,
            Some("600"),
        );
    }
    Ok(svg.finish(20.0))
}

fn trim_number(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_and_legend() {
        let lines: Vec<String> = ["  \"Dogs\" : 386", "  \"Cats\" : 85.5", "  \"Rats\" : 15"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let svg = String::from_utf8(render("showData title Pets", &lines, None).unwrap()).unwrap();
        assert!(svg.contains(">Pets<"), "{svg}");
        assert!(svg.contains(">Dogs [386]<"), "{svg}");
        assert!(svg.contains(">Cats [85.5]<"), "{svg}");
        assert_eq!(svg.matches("<path").count(), 3);
        assert!(render("", &[], None).is_err());
    }
}
