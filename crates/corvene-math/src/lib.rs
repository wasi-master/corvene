//! TeX math typesetting for the diff views (`1331-notebook-diff`,
//! `1332-latex-diff`) and markdown (`1333-markdown-math`).
//!
//! GitHub Desktop renders no math anywhere (its markdown is `marked` without
//! a math extension, and notebooks and `.tex` files are text diffs). github.com
//! typesets `$…$` with MathJax; this crate stands in for it with RaTeX, a pure
//! Rust port of KaTeX: parse → layout → display list → tiny-skia raster.
//!
//! [`render`] returns a straight-alpha BGRA raster (what GPUI image sprites
//! take) painted in the caller's text colour, so formulas follow the theme;
//! explicit `\color{…}` stays as written. Results are cached: notebooks repeat
//! formulas between their two sides and the UI re-asks on every scale change.

use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock};

use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_render::{RenderOptions, render_to_png};
use ratex_types::color::Color;
use ratex_types::display_item::DisplayItem;
use ratex_types::math_style::MathStyle;

/// Formulas longer than this are not typeset (a pasted data dump between two
/// `$` signs, not math).
pub const MAX_TEX_LEN: usize = 16 * 1024;
/// Largest raster in device pixels.
const MAX_PIXELS: u64 = 24 * 1024 * 1024;
/// Logical padding around the ink, in px (keeps antialiased edges).
const PAD: f32 = 1.0;
const CACHE_ENTRIES: usize = 768;

/// Inline (`$…$`, text style) or display (`$$…$$`, display style) math.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Inline,
    Display,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum MathError {
    /// The TeX did not parse; the message is KaTeX's wording.
    #[error("{0}")]
    Parse(String),
    #[error("formula is too long to typeset")]
    TooLong,
    #[error("formula is too large to draw")]
    TooLarge,
    #[error("could not draw formula: {0}")]
    Render(String),
}

/// A typeset formula: `bgra` holds `width * height` straight-alpha pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct MathRaster {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Distance from the raster's top edge to the math baseline, in device px.
    pub baseline: f32,
    /// Device pixels per logical pixel the raster was drawn at.
    pub scale: f32,
}

impl MathRaster {
    /// Logical size in px.
    pub fn size(&self) -> (f32, f32) {
        (
            self.width as f32 / self.scale,
            self.height as f32 / self.scale,
        )
    }

    /// Logical distance from the top to the baseline.
    pub fn logical_baseline(&self) -> f32 {
        self.baseline / self.scale
    }
}

/// User macro definitions in effect for a document: the `\newcommand`,
/// `\renewcommand`, `\def` and `\DeclareMathOperator` lines of a LaTeX
/// preamble, or of every markdown cell of a notebook (Jupyter's MathJax keeps
/// definitions from one cell for the next).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Macros {
    preamble: String,
}

impl Macros {
    pub fn is_empty(&self) -> bool {
        self.preamble.is_empty()
    }

    /// Collects macro definitions from `text` (TeX source, or markdown with
    /// math in it). Later definitions win, as in TeX.
    pub fn collect(text: &str) -> Self {
        let mut out = Self::default();
        out.extend(text);
        out
    }

    pub fn extend(&mut self, text: &str) {
        for def in definitions(text) {
            if !self.preamble.is_empty() {
                self.preamble.push(' ');
            }
            self.preamble.push_str(&def);
        }
    }

    fn hash_key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.preamble.hash(&mut h);
        h.finish()
    }
}

/// The macro-defining commands RaTeX understands, plus `\DeclareMathOperator`,
/// which it does not and which is rewritten to `\operatorname`.
fn definitions(text: &str) -> Vec<String> {
    const COMMANDS: [&str; 7] = [
        "\\newcommand",
        "\\renewcommand",
        "\\providecommand",
        "\\DeclareMathOperator",
        "\\def",
        "\\gdef",
        "\\let",
    ];
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(off) = text[i..].find('\\') {
        let start = i + off;
        let Some(cmd) = COMMANDS.iter().find(|c| {
            text[start..].starts_with(**c)
                && !bytes
                    .get(start + c.len())
                    .is_some_and(|b| b.is_ascii_alphabetic())
        }) else {
            i = start + 1;
            continue;
        };
        let mut j = start + cmd.len();
        let star = bytes.get(j) == Some(&b'*');
        if star {
            j += 1;
        }
        let Some(end) = definition_end(text, j, cmd) else {
            i = start + cmd.len();
            continue;
        };
        if *cmd == "\\DeclareMathOperator" {
            let mut groups = brace_groups(&text[j..end]);
            if let (Some(name), Some(body)) = (groups.next(), groups.next()) {
                let op = if star {
                    "\\operatorname*"
                } else {
                    "\\operatorname"
                };
                out.push(format!("\\newcommand{{{name}}}{{{op}{{{body}}}}}"));
            }
        } else if matches!(*cmd, "\\newcommand" | "\\renewcommand") {
            // RaTeX, like KaTeX, refuses `\newcommand` for a name it already
            // knows (`\R` is built in) and `\renewcommand` for one it does not;
            // TeX documents and notebooks do both freely. Provide, then renew.
            // An optional-argument default (`[n][default]`) is not supported
            // and is dropped.
            if let Some((name, nargs, body)) = command_parts(&text[j..end]) {
                let args = nargs.map(|n| format!("[{n}]")).unwrap_or_default();
                out.push(format!(
                    "\\providecommand{{{name}}}{{}}\\renewcommand{{{name}}}{args}{{{body}}}"
                ));
            }
        } else {
            out.push(text[start..end].to_string());
        }
        i = end;
    }
    out
}

/// End of a definition starting after the command name at `j`: the macro name
/// (`{\foo}` or `\foo`), an optional `[n]` and `[default]`, a `#1#2` parameter
/// text for `\def`, then one balanced `{body}` group (`\let\a\b` has none).
fn definition_end(text: &str, mut j: usize, cmd: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let skip_ws = |j: &mut usize| {
        while bytes.get(*j).is_some_and(|b| b.is_ascii_whitespace()) {
            *j += 1;
        }
    };
    skip_ws(&mut j);
    match bytes.get(j)? {
        b'{' => j = group_end(text, j)?,
        b'\\' => {
            j += 1;
            let name_start = j;
            while bytes
                .get(j)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'@')
            {
                j += 1;
            }
            if j == name_start {
                j += 1;
            }
        }
        _ => return None,
    }
    if cmd == "\\let" {
        skip_ws(&mut j);
        if bytes.get(j) == Some(&b'=') {
            j += 1;
            skip_ws(&mut j);
        }
        if bytes.get(j) != Some(&b'\\') {
            return None;
        }
        j += 1;
        while bytes.get(j).is_some_and(|b| b.is_ascii_alphabetic()) {
            j += 1;
        }
        return Some(j);
    }
    loop {
        skip_ws(&mut j);
        match bytes.get(j)? {
            b'[' => j = text[j..].find(']').map(|e| j + e + 1)?,
            b'#' => j += 2,
            b'{' => return group_end(text, j),
            _ => return None,
        }
    }
}

/// Index just past the `}` matching the `{` at `open`.
fn group_end(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (k, b) in text.as_bytes()[open..].iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' => escaped = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + k + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Name, argument count and body of `{\\name}[n][default]{body}` or
/// `\\name[n]{body}`.
fn command_parts(text: &str) -> Option<(&str, Option<&str>, &str)> {
    let text = text.trim_start();
    let (name, mut rest) = if text.starts_with('{') {
        let end = group_end(text, 0)?;
        (text[1..end - 1].trim(), &text[end..])
    } else {
        let len = 1 + text[1..]
            .find(|c: char| !(c.is_ascii_alphabetic() || c == '@'))
            .unwrap_or(text.len() - 1)
            .max(1);
        (&text[..len], &text[len..])
    };
    let mut nargs = None;
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix('[') {
            let close = r.find(']')?;
            if nargs.is_none() {
                nargs = Some(r[..close].trim());
            }
            rest = &r[close + 1..];
        } else {
            break;
        }
    }
    let end = group_end(rest, 0)?;
    Some((name, nargs, &rest[1..end - 1]))
}

/// The contents of consecutive top-level `{…}` groups in `text`.
fn brace_groups(text: &str) -> impl Iterator<Item = &str> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let open = at + text[at..].find('{')?;
        let end = group_end(text, open)?;
        at = end;
        Some(&text[open + 1..end - 1])
    })
}

/// What to draw: the formula, its mode and the paint parameters.
#[derive(Clone, Debug)]
pub struct Request<'a> {
    pub tex: &'a str,
    pub mode: Mode,
    /// Logical px per em (KaTeX's default is 1.21 × the body text size).
    pub em: f32,
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// Default ink colour as straight RGB.
    pub color: [u8; 3],
    pub macros: &'a Macros,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    tex: String,
    mode: Mode,
    em: u32,
    scale: u32,
    color: [u8; 3],
    macros: u64,
}

type Cached = Result<Arc<MathRaster>, MathError>;

#[derive(Default)]
struct Cache {
    map: HashMap<Key, Cached>,
    order: VecDeque<Key>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// Typesets `req`, from the cache when the same formula was drawn before.
/// CPU-bound (a few ms per formula): call it off the UI thread.
pub fn render(req: &Request<'_>) -> Result<Arc<MathRaster>, MathError> {
    let key = Key {
        tex: req.tex.to_string(),
        mode: req.mode,
        em: req.em.to_bits(),
        scale: req.scale.to_bits(),
        color: req.color,
        macros: req.macros.hash_key(),
    };
    if let Ok(cache) = cache().lock()
        && let Some(hit) = cache.map.get(&key)
    {
        return hit.clone();
    }
    let result = render_uncached(req).map(Arc::new);
    if let Ok(mut cache) = cache().lock() {
        if cache.order.len() >= CACHE_ENTRIES
            && let Some(old) = cache.order.pop_front()
        {
            cache.map.remove(&old);
        }
        cache.order.push_back(key.clone());
        cache.map.insert(key, result.clone());
    }
    result
}

/// The cached result for `req` without drawing, so the UI can paint a
/// formula synchronously when it was typeset before.
pub fn cached(req: &Request<'_>) -> Option<Result<Arc<MathRaster>, MathError>> {
    let key = Key {
        tex: req.tex.to_string(),
        mode: req.mode,
        em: req.em.to_bits(),
        scale: req.scale.to_bits(),
        color: req.color,
        macros: req.macros.hash_key(),
    };
    cache().lock().ok()?.map.get(&key).cloned()
}

/// Whether `tex` parses (no drawing). Used to decide if `$…$` in prose is math.
pub fn parses(tex: &str, macros: &Macros) -> bool {
    tex.len() <= MAX_TEX_LEN && parse(tex, macros).is_ok()
}

fn parse(tex: &str, macros: &Macros) -> Result<Vec<ratex_parser::ParseNode>, MathError> {
    let source = if macros.is_empty() {
        tex.to_string()
    } else {
        format!("{} {}", macros.preamble, tex)
    };
    catch_unwind(AssertUnwindSafe(|| ratex_parser::parse(&source)))
        .map_err(|_| MathError::Parse("the formula could not be parsed".into()))?
        .map_err(|e| MathError::Parse(e.message))
}

fn render_uncached(req: &Request<'_>) -> Result<MathRaster, MathError> {
    let tex = req.tex.trim();
    if tex.len() > MAX_TEX_LEN {
        return Err(MathError::TooLong);
    }
    let nodes = parse(tex, req.macros)?;
    let options = LayoutOptions {
        style: match req.mode {
            Mode::Inline => MathStyle::Text,
            Mode::Display => MathStyle::Display,
        },
        ..LayoutOptions::default()
    };
    let mut list = catch_unwind(AssertUnwindSafe(|| {
        to_display_list(&layout(&nodes, &options))
    }))
    .map_err(|_| MathError::Render("layout failed".into()))?;

    let scale = req.scale.clamp(0.5, 8.0);
    let em_px = (req.em * scale) as f64;
    let w = (list.width * em_px) as u64 + 4;
    let h = (list.total_height() * em_px) as u64 + 4;
    if w * h > MAX_PIXELS {
        return Err(MathError::TooLarge);
    }

    let [r, g, b] = req.color;
    let ink = Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    for item in &mut list.items {
        let color = match item {
            DisplayItem::GlyphPath { color, .. }
            | DisplayItem::Line { color, .. }
            | DisplayItem::Rect { color, .. }
            | DisplayItem::Path { color, .. } => color,
        };
        if *color == Color::BLACK {
            *color = ink;
        }
    }

    let options = RenderOptions {
        font_size: req.em,
        padding: PAD,
        background_color: Color::new(0.0, 0.0, 0.0, 0.0),
        font_dir: String::new(),
        device_pixel_ratio: scale,
    };
    let png = catch_unwind(AssertUnwindSafe(|| render_to_png(&list, &options)))
        .map_err(|_| MathError::Render("rasteriser failed".into()))?
        .map_err(MathError::Render)?;
    let pixmap =
        tiny_skia::Pixmap::decode_png(&png).map_err(|e| MathError::Render(e.to_string()))?;

    let mut bgra = Vec::with_capacity(pixmap.pixels().len() * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        bgra.extend_from_slice(&[c.blue(), c.green(), c.red(), c.alpha()]);
    }
    Ok(MathRaster {
        bgra,
        width: pixmap.width(),
        height: pixmap.height(),
        baseline: (PAD as f64 * scale as f64 + list.height * em_px) as f32,
        scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(tex: &'a str, mode: Mode, macros: &'a Macros) -> Request<'a> {
        Request {
            tex,
            mode,
            em: 16.0,
            scale: 2.0,
            color: [10, 20, 30],
            macros,
        }
    }

    fn ink(raster: &MathRaster) -> usize {
        raster.bgra.chunks(4).filter(|p| p[3] > 0).count()
    }

    #[test]
    fn typesets_a_fraction() {
        let m = Macros::default();
        let r = render(&req(r"\frac{-b \pm \sqrt{b^2-4ac}}{2a}", Mode::Display, &m)).unwrap();
        assert!(r.width > 40 && r.height > 40, "{}x{}", r.width, r.height);
        assert_eq!(r.bgra.len(), (r.width * r.height * 4) as usize);
        assert!(ink(&r) > 100);
        assert!(r.baseline > 0.0 && r.baseline < r.height as f32);
        // default ink is painted in the requested colour
        let p = r.bgra.chunks(4).find(|p| p[3] == 255).unwrap();
        assert_eq!(&p[..3], &[30, 20, 10]);
    }

    #[test]
    fn display_is_taller_than_inline() {
        let m = Macros::default();
        let d = render(&req(r"\sum_{i=0}^n i", Mode::Display, &m)).unwrap();
        let i = render(&req(r"\sum_{i=0}^n i", Mode::Inline, &m)).unwrap();
        assert!(d.height > i.height);
    }

    #[test]
    fn explicit_colours_survive() {
        let m = Macros::default();
        let r = render(&req(r"\color{red}{x}", Mode::Inline, &m)).unwrap();
        assert!(
            r.bgra
                .chunks(4)
                .any(|p| p[3] == 255 && p[2] > 200 && p[0] < 50)
        );
    }

    #[test]
    fn reports_parse_errors() {
        let m = Macros::default();
        let err = render(&req(r"\frac{1}{", Mode::Inline, &m)).unwrap_err();
        assert!(matches!(err, MathError::Parse(_)));
        assert!(!parses(r"\undefinedthing{x}", &m));
        assert!(parses(r"x^2", &m));
    }

    #[test]
    fn user_macros_apply() {
        let m = Macros::collect(
            r"\newcommand{\R}{\mathbb{R}} \DeclareMathOperator*{\Argmx}{arg\,max} \def\eps{\varepsilon}",
        );
        assert!(parses(r"x \in \R", &m));
        assert!(parses(r"\Argmx_x f(x)", &m));
        assert!(parses(r"\eps", &m));
        assert!(parses(
            r"\R^n",
            &Macros::collect(r"\renewcommand{\R}{\mathbb{R}}")
        ));
        assert!(!parses(r"\Argmx_x", &Macros::default()));
    }

    #[test]
    fn collects_definitions_with_arguments() {
        let m = Macros::collect(r"text \newcommand{\norm}[1]{\left\| #1 \right\|} more");
        assert!(parses(r"\norm{x}", &m));
        assert_eq!(definitions(r"\newcommand\foo{x}").len(), 1);
        assert_eq!(definitions(r"\newcommandx{x}").len(), 0);
    }

    #[test]
    fn environments_render() {
        let m = Macros::default();
        for tex in [
            r"\begin{aligned} a &= b \\ c &= d \end{aligned}",
            r"\begin{pmatrix} 1 & 0 \\ 0 & 1 \end{pmatrix}",
            r"f(x) = \begin{cases} 0 & x < 0 \\ 1 & \text{otherwise} \end{cases}",
        ] {
            assert!(render(&req(tex, Mode::Display, &m)).is_ok(), "{tex}");
        }
    }
}
