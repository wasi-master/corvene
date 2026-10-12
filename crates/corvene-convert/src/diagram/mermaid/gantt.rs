//! Mermaid gantt charts: sections, tasks with start dates or `after`
//! dependencies, end dates or durations (`3d`, `1w`, `12h`), the `done`,
//! `active`, `crit` and `milestone` tags, `dateFormat`. Drawn on a dated
//! axis. `excludes` (skipped weekends) is not applied.

use std::collections::HashMap;

use super::super::svg::{Anchor, Style, Svg, colors, text_width};
use super::label;

#[derive(Clone, Debug)]
struct Task {
    name: String,
    section: usize,
    start: f64,
    end: f64,
    done: bool,
    active: bool,
    crit: bool,
    milestone: bool,
}

/// A date in `format` (dayjs tokens: `YYYY`, `YY`, `MM`, `M`, `DD`, `D`,
/// `HH`, `H`, `mm`, `ss`; `X` Unix seconds, `x` milliseconds) as Unix
/// seconds (UTC).
fn parse_date(value: &str, format: &str) -> Option<f64> {
    let value = value.trim();
    if format == "X" {
        return value.parse().ok();
    }
    if format == "x" {
        return value.parse::<f64>().ok().map(|ms| ms / 1000.0);
    }
    let (mut y, mut mo, mut d, mut h, mut mi, mut s) = (1970i64, 1i64, 1i64, 0i64, 0i64, 0i64);
    let f = format.as_bytes();
    let v = value.as_bytes();
    let (mut i, mut j) = (0, 0);
    let token = |f: &[u8], i: usize| -> (u8, usize) {
        let c = f[i];
        let mut k = i;
        while k < f.len() && f[k] == c {
            k += 1;
        }
        (c, k - i)
    };
    let digits = |v: &[u8], j: usize, max: usize| -> Option<(i64, usize)> {
        let mut k = j;
        while k < v.len() && k - j < max && v[k].is_ascii_digit() {
            k += 1;
        }
        if k == j {
            return None;
        }
        std::str::from_utf8(&v[j..k])
            .ok()?
            .parse()
            .ok()
            .map(|n| (n, k))
    };
    while i < f.len() {
        let (c, len) = token(f, i);
        match c {
            b'Y' => {
                let (n, k) = digits(v, j, if len >= 4 { 4 } else { 2 })?;
                y = if len >= 4 { n } else { 2000 + n };
                j = k;
            }
            b'M' if len <= 2 => {
                let (n, k) = digits(v, j, 2)?;
                mo = n;
                j = k;
            }
            b'D' if len <= 2 => {
                let (n, k) = digits(v, j, 2)?;
                d = n;
                j = k;
            }
            b'H' | b'h' => {
                let (n, k) = digits(v, j, 2)?;
                h = n;
                j = k;
            }
            b'm' => {
                let (n, k) = digits(v, j, 2)?;
                mi = n;
                j = k;
            }
            b's' => {
                let (n, k) = digits(v, j, 2)?;
                s = n;
                j = k;
            }
            _ => {
                // a literal separator
                for _ in 0..len {
                    if v.get(j) != Some(&c) {
                        return None;
                    }
                    j += 1;
                }
            }
        }
        i += len;
    }
    if j != v.len() || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s) as f64)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `3d`, `1.5w`, `12h`, `30m` in seconds.
fn duration(value: &str) -> Option<f64> {
    let v = value.trim();
    let split = v.find(|c: char| c.is_ascii_alphabetic())?;
    let n: f64 = v[..split].trim().parse().ok()?;
    let unit = match &v[split..] {
        "ms" => 0.001,
        "s" => 1.0,
        "m" | "min" => 60.0,
        "h" => 3600.0,
        "d" => 86_400.0,
        "w" => 7.0 * 86_400.0,
        "M" => 30.0 * 86_400.0,
        "y" => 365.0 * 86_400.0,
        _ => return None,
    };
    Some(n * unit)
}

pub fn render(lines: &[String], title: Option<String>) -> Result<Vec<u8>, String> {
    let mut format = "YYYY-MM-DD".to_string();
    let mut title = title;
    let mut sections: Vec<String> = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut last_end: Option<f64> = None;
    for line in lines {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        let word = s.split_whitespace().next().unwrap_or("");
        let rest = s[word.len()..].trim();
        match word {
            "dateFormat" => {
                format = rest.to_string();
                continue;
            }
            "title" => {
                title = Some(rest.to_string());
                continue;
            }
            "section" => {
                sections.push(label(rest));
                continue;
            }
            "axisFormat" | "excludes" | "includes" | "todayMarker" | "tickInterval" | "weekday"
            | "inclusiveEndDates" | "topAxis" | "displayMode" | "accTitle:" | "accDescr:"
            | "click" => continue,
            _ => {}
        }
        let Some((name, spec)) = s.split_once(':') else {
            continue;
        };
        let mut parts: Vec<&str> = spec
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let mut task = Task {
            name: label(name),
            section: sections.len().saturating_sub(1),
            start: 0.0,
            end: 0.0,
            done: false,
            active: false,
            crit: false,
            milestone: false,
        };
        while let Some(first) = parts.first() {
            match *first {
                "done" => task.done = true,
                "active" => task.active = true,
                "crit" => task.crit = true,
                "milestone" => task.milestone = true,
                _ => break,
            }
            parts.remove(0);
        }
        let is_start = |p: &str| p.starts_with("after ") || parse_date(p, &format).is_some();
        let (id, start_spec, end_spec) = match parts.len() {
            0 => continue,
            1 => (None, None, parts[0]),
            2 if is_start(parts[0]) => (None, Some(parts[0]), parts[1]),
            2 => (Some(parts[0]), None, parts[1]),
            _ => (Some(parts[0]), Some(parts[1]), parts[2]),
        };
        let start = match start_spec {
            Some(p) if p.starts_with("after ") => p[6..]
                .split_whitespace()
                .filter_map(|id| ids.get(id).map(|&t| tasks[t].end))
                .fold(None, |acc: Option<f64>, e| {
                    Some(acc.map_or(e, |a| a.max(e)))
                }),
            Some(p) => parse_date(p, &format),
            None => last_end,
        };
        let Some(start) = start else {
            continue;
        };
        let end = match end_spec {
            p if p.starts_with("until ") => p[6..]
                .split_whitespace()
                .filter_map(|id| ids.get(id).map(|&t| tasks[t].start))
                .fold(None, |acc: Option<f64>, e| {
                    Some(acc.map_or(e, |a| a.min(e)))
                }),
            p => parse_date(p, &format).or_else(|| duration(p).map(|d| start + d)),
        };
        let Some(end) = end else {
            continue;
        };
        task.start = start;
        task.end = end.max(start);
        last_end = Some(task.end);
        if let Some(id) = id {
            ids.insert(id.to_string(), tasks.len());
        }
        tasks.push(task);
        if tasks.len() > 2000 {
            break;
        }
    }
    if tasks.is_empty() {
        return Err("The gantt chart has no tasks it could place.".into());
    }
    let t0 = tasks.iter().map(|t| t.start).fold(f64::INFINITY, f64::min);
    let t1 = tasks
        .iter()
        .map(|t| t.end)
        .fold(f64::NEG_INFINITY, f64::max)
        .max(t0 + 86_400.0);
    let label_w = tasks
        .iter()
        .map(|t| text_width(&t.name, 13.0))
        .chain(sections.iter().map(|s| text_width(s, 13.0)))
        .fold(80.0, f64::max)
        + 24.0;
    let width = 900.0;
    let scale = width / (t1 - t0);
    let x_of = |t: f64| label_w + (t - t0) * scale;
    let row = 30.0;
    let top = 40.0;
    let mut svg = Svg::new();
    let bottom = top + tasks.len() as f64 * row;
    // section bands
    let mut ix = 0;
    while ix < tasks.len() {
        let sec = tasks[ix].section;
        let start = ix;
        while ix < tasks.len() && tasks[ix].section == sec {
            ix += 1;
        }
        let y0 = top + start as f64 * row;
        let h = (ix - start) as f64 * row;
        let band = if sec.is_multiple_of(2) {
            "#f6f8fa"
        } else {
            "#ffffff"
        };
        svg.rect(
            0.0,
            y0,
            label_w + width,
            h,
            0.0,
            &Style::new(band, "none", 0.0),
        );
        if let Some(name) = sections.get(sec) {
            svg.text(
                8.0,
                y0 + 12.0,
                name,
                12.0,
                Anchor::Start,
                colors::MUTED,
                Some("600"),
            );
        }
    }
    // ticks
    let span_days = (t1 - t0) / 86_400.0;
    let step_days = [1.0, 2.0, 7.0, 14.0, 30.0, 61.0, 91.0, 182.0, 365.0]
        .into_iter()
        .find(|s| span_days / s <= 12.0)
        .unwrap_or(730.0);
    let first_day = (t0 / 86_400.0).ceil();
    let mut day = first_day;
    while day * 86_400.0 <= t1 {
        let x = x_of(day * 86_400.0);
        svg.line(
            &[(x, top - 6.0), (x, bottom)],
            false,
            &Style::line(colors::CLUSTER_STROKE, 1.0),
            None,
            None,
        );
        let date = crate::compiled::pyc::utc(day as i64 * 86_400);
        svg.text(
            x,
            top - 16.0,
            &date[..10],
            11.0,
            Anchor::Middle,
            colors::MUTED,
            None,
        );
        day += step_days;
    }
    for (i, t) in tasks.iter().enumerate() {
        let y = top + i as f64 * row + row / 2.0;
        let color = if t.crit {
            "#cf222e"
        } else if t.done {
            "#8c959f"
        } else if t.active {
            "#0969da"
        } else {
            colors::SERIES[t.section % colors::SERIES.len()]
        };
        if t.milestone {
            let x = x_of(t.start);
            let r = 8.0;
            svg.polygon(
                &[(x, y - r), (x + r, y), (x, y + r), (x - r, y)],
                &Style::new(color, color, 1.0),
            );
            svg.text(
                x + r + 6.0,
                y,
                &t.name,
                13.0,
                Anchor::Start,
                colors::INK,
                None,
            );
            continue;
        }
        let (x0, x1) = (x_of(t.start), x_of(t.end).max(x_of(t.start) + 2.0));
        let fill = if t.active && !t.crit {
            "#ddf4ff"
        } else {
            color
        };
        svg.rect(
            x0,
            y - 10.0,
            x1 - x0,
            20.0,
            3.0,
            &Style::new(fill, color, 1.2),
        );
        let w = text_width(&t.name, 13.0);
        if w + 10.0 < x1 - x0 {
            let ink = if t.active && !t.crit {
                colors::INK
            } else {
                "#ffffff"
            };
            svg.text((x0 + x1) / 2.0, y, &t.name, 13.0, Anchor::Middle, ink, None);
        } else {
            svg.text(x1 + 6.0, y, &t.name, 13.0, Anchor::Start, colors::INK, None);
        }
    }
    if let Some(t) = title {
        svg.text(
            label_w + width / 2.0,
            0.0,
            &t,
            18.0,
            Anchor::Middle,
            colors::INK,
            Some("600"),
        );
    }
    Ok(svg.finish(16.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_durations() {
        assert_eq!(
            parse_date("2024-01-02", "YYYY-MM-DD"),
            Some(1_704_153_600.0)
        );
        assert_eq!(parse_date("02.01.24", "DD.MM.YY"), Some(1_704_153_600.0));
        assert_eq!(
            parse_date("2024-01-02 10:30", "YYYY-MM-DD HH:mm"),
            Some(1_704_191_400.0)
        );
        assert_eq!(parse_date("2024-13-02", "YYYY-MM-DD"), None);
        assert_eq!(duration("3d"), Some(259_200.0));
        assert_eq!(duration("1w"), Some(604_800.0));
        assert_eq!(duration("x"), None);
    }

    #[test]
    fn tasks_place_on_the_axis() {
        let lines: Vec<String> = "    dateFormat YYYY-MM-DD\n    title Plan\n    section Build\n    Design :done, d1, 2024-01-01, 3d\n    Code :active, c1, after d1, 1w\n    section Ship\n    Release :milestone, after c1, 0d\n    Docs :2024-01-05, 2024-01-12"
            .lines()
            .map(str::to_string)
            .collect();
        let svg = String::from_utf8(render(&lines, None).unwrap()).unwrap();
        for text in [
            ">Plan<",
            ">Build<",
            ">Ship<",
            ">Design<",
            ">Code<",
            ">Release<",
            ">Docs<",
            ">2024-01-01<",
        ] {
            assert!(svg.contains(text), "{text} in {svg}");
        }
    }
}
