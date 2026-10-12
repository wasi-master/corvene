//! Markdown as GPUI text - GHD `ui/lib/sandboxed-markdown.tsx` with
//! `static/common/markdown.css` (Primer's `.markdown-body`).
//!
//! GHD renders `marked` output inside a sandboxed iframe; Corvene parses with
//! `corvene_core::markdown` and lays the blocks out natively the way
//! GitHub's `.markdown-body` does: headings (h1/h2 with the muted bottom
//! border), paragraphs, bold / italic / strikethrough, inline code (mono font
//! on `--md-neutral-muted-color`), fenced code blocks (`pre`: 16 px padding,
//! 85 % mono, horizontal scroll), links in `--md-accent-fg-color` that open
//! in the browser, bullet / numbered / task lists (a disabled checkbox in
//! the marker column), block quotes and GitHub's alerts (`[!NOTE]`: the
//! coloured left border and titled icon), tables (`th, td { padding: 6px
//! 13px; border: 1px solid }`, bold centred head cells, zebra rows, the
//! column alignment), `<details>` (a disclosure triangle on the summary,
//! collapsed unless `open`; a click toggles it), `<p align>` blocks and
//! footnotes (`section.footnotes`: muted, small, under a rule). Images
//! (`max-width: 100%`) load through `corvene_core::markdown_images` and
//! show at their natural size or their `width` / `height`, scaled down to
//! the width there is; their alt text stands in while loading or when they
//! cannot be read. Vertical rhythm follows the CSS margins (16 px between
//! blocks, 24 px above headings and around rules).
//!
//! [`rich_text`] is GHD `ui/lib/rich-text.tsx` over a commit message: links
//! in `--link-button-color`, with the URL as the tooltip when they show
//! `#123` or `@name`.
//!
//! Deviations: text is not selectable (GPUI static text), link hover has no
//! colour change, inline code keeps the paragraph's font size (GHD 85 %)
//! and has no padding or rounded corners, `<sub>` / `<sup>` are not smaller,
//! and images sit on the bottom of their line rather than the baseline.
//!
//! Deviation (`1333-markdown-math`, and always in notebooks and LaTeX
//! files): with [`MarkdownOptions::macros`] set, `$…$` / `$$…$$` math is
//! typeset ([`crate::math`]); notebooks and LaTeX files also get
//! highlighted code blocks, changed-word marks and their own pictures
//! ([`MarkdownOptions`]).
//!
//! Deviation (`881-issue-title-tooltips`): with
//! [`rich_text_with_issue_titles`] a `#123` link's tooltip is "#123 <title>"
//! from the repository's issue cache (hovering loads the cache), else the URL.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use corvene_core::markdown::{
    AlertKind, Align, Block, InlineImage, ListItem, RichText, resolve_link,
};
use std::ops::Range;

use crate::math::SharedMacros;
use corvene_core::{Dispatcher, GitHubRepository};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::theme::sizes::zpx;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};

/// `.markdown-body` spacing: block margin-bottom and heading / rule margins.
#[allow(non_snake_case)]
fn BLOCK_GAP() -> Pixels {
    zpx(16.)
}
#[allow(non_snake_case)]
fn HEADING_GAP() -> Pixels {
    zpx(24.)
}
/// `ul, ol { padding-left: 2em }` at 12 px.
#[allow(non_snake_case)]
fn LIST_INDENT() -> Pixels {
    zpx(24.)
}
/// `li + li { margin-top: .25em }`
#[allow(non_snake_case)]
fn ITEM_GAP() -> Pixels {
    zpx(3.)
}
/// `line-height: 1.5` at 12 px.
#[allow(non_snake_case)]
fn LINE_HEIGHT() -> Pixels {
    zpx(18.)
}

/// Parsed documents by source text, so views that keep only the Markdown
/// do not parse it again every frame. Small: the bodies on screen.
const PARSE_CACHE_SIZE: usize = 64;

/// The parsed documents by text, and their texts oldest first.
type ParseCache = (HashMap<Arc<str>, Arc<Vec<Block>>>, VecDeque<Arc<str>>);

thread_local! {
    static PARSED: RefCell<ParseCache> = RefCell::new((HashMap::new(), VecDeque::new()));
}

/// `corvene_core::markdown::parse(text)`, from a cache of the documents
/// shown lately.
pub fn parsed(text: &str) -> Arc<Vec<Block>> {
    PARSED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(blocks) = cache.0.get(text) {
            return blocks.clone();
        }
        let blocks = Arc::new(corvene_core::markdown::parse(text));
        let key: Arc<str> = Arc::from(text);
        if cache.1.len() >= PARSE_CACHE_SIZE
            && let Some(old) = cache.1.pop_front()
        {
            cache.0.remove(&old);
        }
        cache.1.push_back(key.clone());
        cache.0.insert(key, blocks.clone());
        blocks
    })
}

/// [`parsed`] with math (`1333-markdown-math`).
pub fn parsed_math(text: &str) -> Arc<Vec<Block>> {
    thread_local! {
        static PARSED_MATH: RefCell<ParseCache> = RefCell::new((HashMap::new(), VecDeque::new()));
    }
    PARSED_MATH.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(blocks) = cache.0.get(text) {
            return blocks.clone();
        }
        let blocks = Arc::new(corvene_core::markdown::parse_with(
            text,
            corvene_core::markdown::ParseOptions { math: true },
        ));
        let key: Arc<str> = Arc::from(text);
        if cache.1.len() >= PARSE_CACHE_SIZE
            && let Some(old) = cache.1.pop_front()
        {
            cache.0.remove(&old);
        }
        cache.1.push_back(key.clone());
        cache.0.insert(key, blocks.clone());
        blocks
    })
}

/// Pictures a document carries itself (a notebook cell's attachments, a
/// repository file at the diff's revision), by source as written.
pub type ImageResolver = Rc<dyn Fn(&str) -> Option<Arc<Image>>>;

/// How [`markdown_with`] draws a document beyond GHD's renderer.
#[derive(Clone, Default)]
pub struct MarkdownOptions {
    pub base_href: Option<String>,
    /// Typeset math with these macros (`None`: math shows as its source).
    pub macros: Option<SharedMacros>,
    /// Changed-word ranges of a lone paragraph or heading, and their colour.
    pub marks: Option<(Rc<[Range<usize>]>, Hsla)>,
    /// Pictures resolved before the network is tried.
    pub images: Option<ImageResolver>,
    /// Syntax-highlight fenced code blocks by their language.
    pub highlight_code: bool,
    /// The body text size (GHD's 12 px when `None`).
    pub font_size: Option<f32>,
}

/// `1333-markdown-math`: the options GitHub.com-like Markdown (issues, pull
/// requests, releases) is drawn with when the flag is on.
pub fn github_options(cx: &App) -> Option<MarkdownOptions> {
    let on = corvene_core::AppState::try_global(cx).is_some_and(|s| {
        s.read(cx)
            .flags
            .bool(corvene_core::flags::ids::MARKDOWN_MATH)
    });
    on.then(|| MarkdownOptions {
        macros: Some(SharedMacros::default()),
        ..MarkdownOptions::default()
    })
}

/// `markdown` with `1333-markdown-math` applied: the text is parsed with
/// math when the flag is on.
pub fn github_markdown(
    id: impl Into<SharedString>,
    text: &str,
    base_href: Option<&str>,
    cx: &App,
) -> Div {
    match github_options(cx) {
        Some(mut options) => {
            options.base_href = base_href.map(str::to_string);
            markdown_with(id, &parsed_math(text), &options, cx)
        }
        None => markdown(id, &parsed(text), base_href, cx),
    }
}

/// `<details>` blocks toggled away from their initial state (`open` or
/// not), by element key; kept for the session.
#[derive(Default)]
struct DetailsToggles(HashSet<SharedString>);

impl Global for DetailsToggles {}

/// Render `blocks` at `font_size` (GHD `--font-size`, 12 px) with
/// `line-height: 1.5`. `base_href` resolves relative links and image
/// sources (GHD `baseHref`). `id` keys the collapsible state: the same
/// document under the same id keeps its `<details>` open or closed.
pub fn markdown(
    id: impl Into<SharedString>,
    blocks: &[Block],
    base_href: Option<&str>,
    cx: &App,
) -> Div {
    markdown_with(
        id,
        blocks,
        &MarkdownOptions {
            base_href: base_href.map(str::to_string),
            ..MarkdownOptions::default()
        },
        cx,
    )
}

/// [`markdown`] with [`MarkdownOptions`].
pub fn markdown_with(
    id: impl Into<SharedString>,
    blocks: &[Block],
    options: &MarkdownOptions,
    cx: &App,
) -> Div {
    let underline =
        corvene_core::AppState::try_global(cx).is_some_and(|s| s.read(cx).settings.underline_links);
    let size = options.font_size.unwrap_or(12.);
    let r = Renderer {
        id: id.into(),
        base: options.base_href.as_deref().map(Rc::from),
        t: cx.ghd(),
        cx,
        underline,
        link_buttons: false,
        issues: None,
        next: Cell::new(0),
        next_details: Cell::new(0),
        align: Cell::new(None),
        macros: options.macros.clone(),
        marks: options.marks.clone(),
        images: options.images.clone(),
        highlight_code: options.highlight_code,
        font_size: size,
    };
    div()
        .text_size(zpx(size))
        .line_height(zpx(size * 1.5))
        .text_color(r.t.text)
        .child(r.blocks(blocks, 0))
}

/// One [`RichText`] run (styled spans, clickable links) without block
/// layout, in the surrounding text style.
pub fn rich_text(id: impl Into<SharedString>, text: &RichText, cx: &App) -> AnyElement {
    rich_text_with_issue_titles(id, text, None, cx)
}

/// [`rich_text`] whose `#123` links into `issues` (`881-issue-title-tooltips`)
/// show the issue's title as their tooltip.
pub fn rich_text_with_issue_titles(
    id: impl Into<SharedString>,
    text: &RichText,
    issues: Option<GitHubRepository>,
    cx: &App,
) -> AnyElement {
    let underline =
        corvene_core::AppState::try_global(cx).is_some_and(|s| s.read(cx).settings.underline_links);
    Renderer {
        id: id.into(),
        base: None,
        t: cx.ghd(),
        cx,
        underline,
        link_buttons: true,
        issues: issues.map(Rc::new),
        next: Cell::new(0),
        next_details: Cell::new(0),
        align: Cell::new(None),
        macros: None,
        marks: None,
        images: None,
        highlight_code: false,
        font_size: 12.,
    }
    .rich(text)
}

/// `881-issue-title-tooltips`: "#N title" for issue `number` of `github` from
/// the issue cache; a cache not read yet is loaded for the next hover.
fn issue_title(github: &Rc<GitHubRepository>, number: u64, cx: &mut App) -> Option<String> {
    let state = corvene_core::AppState::try_global(cx)?;
    let key = corvene_core::autocomplete::cache_key(github);
    let cache = state.read(cx).issues.get(&key);
    if let Some(issue) = cache.and_then(|c| c.issues.iter().find(|i| i.number == number)) {
        return Some(format!("#{number} {}", issue.title));
    }
    if !cache.is_some_and(|c| c.loaded) {
        let github = github.clone();
        cx.defer(move |cx| Dispatcher::refresh_issues(&github, cx));
    }
    None
}

/// The issue number a `#123` link of `github` points at.
fn issue_number(text: &str, href: &str, github: &GitHubRepository) -> Option<u64> {
    let number: u64 = text.strip_prefix('#')?.parse().ok()?;
    let prefix = format!("{}/issues/", github.html_url).to_lowercase();
    let rest = href.to_lowercase().strip_prefix(&prefix)?.to_string();
    (rest.parse::<u64>().ok()? == number).then_some(number)
}

/// GitHub's alert colours (`--borderColor-*-emphasis`, `--fgColor-*`) and
/// octicon per kind.
fn alert_style(kind: AlertKind, dark: bool) -> (Hsla, Hsla, Octicon) {
    let c = crate::theme::c;
    match (kind, dark) {
        (AlertKind::Note, false) => (c(0x0969da), c(0x0969da), Octicon::Info),
        (AlertKind::Note, true) => (c(0x1f6feb), c(0x4493f8), Octicon::Info),
        (AlertKind::Tip, false) => (c(0x1a7f37), c(0x1a7f37), Octicon::LightBulb),
        (AlertKind::Tip, true) => (c(0x238636), c(0x3fb950), Octicon::LightBulb),
        (AlertKind::Important, false) => (c(0x8250df), c(0x8250df), Octicon::Comment),
        (AlertKind::Important, true) => (c(0x8957e5), c(0xab7df8), Octicon::Comment),
        (AlertKind::Warning, false) => (c(0x9a6700), c(0x9a6700), Octicon::Alert),
        (AlertKind::Warning, true) => (c(0x9e6a03), c(0xd29922), Octicon::Alert),
        (AlertKind::Caution, false) => (c(0xcf222e), c(0xd1242f), Octicon::Stop),
        (AlertKind::Caution, true) => (c(0xda3633), c(0xf85149), Octicon::Stop),
    }
}

/// One piece of a line of a paragraph with images or formulas.
enum Piece {
    Text(std::ops::Range<usize>),
    Image(usize),
    Math(usize),
}

/// A paragraph with images as lines (split at its line breaks) of text
/// runs and images.
fn lines_of(text: &RichText) -> Vec<Vec<Piece>> {
    let mut lines: Vec<Vec<Piece>> = vec![Vec::new()];
    let push_text = |lines: &mut Vec<Vec<Piece>>, from: usize, to: usize| {
        let mut start = from;
        for (ix, _) in text.text[from..to].match_indices('\n') {
            let at = from + ix;
            if at > start
                && let Some(line) = lines.last_mut()
            {
                line.push(Piece::Text(start..at));
            }
            lines.push(Vec::new());
            start = at + 1;
        }
        if to > start
            && let Some(line) = lines.last_mut()
        {
            line.push(Piece::Text(start..to));
        }
    };
    // images and formulas in text order
    let mut objects: Vec<(usize, Piece)> = text
        .images
        .iter()
        .enumerate()
        .map(|(ix, i)| (i.at, Piece::Image(ix)))
        .chain(
            text.maths
                .iter()
                .enumerate()
                .map(|(ix, m)| (m.at, Piece::Math(ix))),
        )
        .collect();
    objects.sort_by_key(|(at, _)| *at);
    let mut at = 0;
    for (object_at, piece) in objects {
        let object_at = object_at.clamp(at, text.text.len());
        push_text(&mut lines, at, object_at);
        let display = matches!(piece, Piece::Math(ix) if text.maths[ix].display);
        // `$$…$$` amid text sits on a line of its own
        if display && lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(Vec::new());
        }
        if let Some(line) = lines.last_mut() {
            line.push(piece);
        }
        if display {
            lines.push(Vec::new());
        }
        at = object_at;
    }
    push_text(&mut lines, at, text.text.len());
    lines
}

/// The size an image shows at in CSS pixels: its `width` / `height`
/// attributes (the other from the aspect ratio), else its natural size.
fn image_box(image: &InlineImage, natural: Option<(u32, u32)>) -> Option<(f32, f32)> {
    let ratio = natural.map(|(w, h)| w as f32 / h.max(1) as f32);
    match (image.width, image.height, natural, ratio) {
        (Some(w), Some(h), _, _) => Some((w as f32, h as f32)),
        (Some(w), None, _, Some(r)) => Some((w as f32, w as f32 / r)),
        (None, Some(h), _, Some(r)) => Some((h as f32 * r, h as f32)),
        (None, None, Some((w, h)), _) => Some((w as f32, h as f32)),
        _ => None,
    }
}

struct Renderer<'a> {
    id: SharedString,
    base: Option<Rc<str>>,
    t: &'a GhdTheme,
    cx: &'a App,
    underline: bool,
    /// GHD `RichText`'s `LinkButton`s: `--link-button-color`, and the URL as
    /// the title when the text differs (`#123`, `@name`).
    link_buttons: bool,
    /// `881-issue-title-tooltips`: whose issue titles `#123` links show.
    issues: Option<Rc<GitHubRepository>>,
    next: Cell<usize>,
    next_details: Cell<usize>,
    /// The `<p align>` around the blocks being drawn.
    align: Cell<Option<Align>>,
    /// [`MarkdownOptions`]
    macros: Option<SharedMacros>,
    marks: Option<(Rc<[Range<usize>]>, Hsla)>,
    images: Option<ImageResolver>,
    highlight_code: bool,
    font_size: f32,
}

impl Renderer<'_> {
    fn element_id(&self) -> ElementId {
        let n = self.next.get();
        self.next.set(n + 1);
        ElementId::Name(format!("{}-md-{n}", self.id).into())
    }

    /// A column of blocks with collapsed CSS margins between them.
    fn blocks(&self, blocks: &[Block], list_depth: usize) -> Div {
        let mut col = div().flex().flex_col().min_w_0();
        let mut prev: Option<&Block> = None;
        for block in blocks {
            let gap = match (prev, block) {
                (None, _) => zpx(0.),
                // `ul ul, ol ol … { margin: 0 }` inside a list item
                (Some(_), Block::List { .. }) if list_depth > 0 => zpx(0.),
                (Some(_), Block::Heading { .. } | Block::Rule) | (Some(Block::Rule), _) => {
                    HEADING_GAP()
                }
                _ => BLOCK_GAP(),
            };
            col = col.child(div().mt(gap).child(self.block(block, list_depth)));
            prev = Some(block);
        }
        col
    }

    fn block(&self, block: &Block, list_depth: usize) -> AnyElement {
        let t = self.t;
        match block {
            Block::Paragraph(text) => self.paragraph(text),
            Block::Heading { level, text } => {
                // h1 2em, h2 1.5em, h3 1.25em, h4 1em, h5 .875em, h6 .85em
                let size = match level {
                    1 => 24.,
                    2 => 18.,
                    3 => 15.,
                    4 => 12.,
                    5 => 10.5,
                    _ => 10.2,
                };
                div()
                    .text_size(zpx(size))
                    .line_height(zpx(size * 1.25))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(*level == 6, |d| d.text_color(t.md_fg_muted))
                    .when(*level <= 2, |d| {
                        d.pb(zpx(size * 0.3))
                            .border_b_1()
                            .border_color(t.md_border_muted)
                    })
                    .child(self.paragraph(text))
                    .into_any_element()
            }
            Block::CodeBlock { code, language } if self.highlight_code => {
                self.highlighted_code(code, language.as_deref())
            }
            Block::Math { tex, number } => match &self.macros {
                Some(macros) => crate::math::block(
                    self.element_id(),
                    tex,
                    number.as_deref(),
                    macros,
                    zpx(self.font_size),
                    self.cx,
                ),
                None => div()
                    .font_family(mono_font())
                    .text_color(t.md_fg_muted)
                    .child(SharedString::from(format!("$$ {tex} $$")))
                    .into_any_element(),
            },
            Block::CodeBlock { code, .. } => div()
                .id(self.element_id())
                .w_full()
                .overflow_x_scroll()
                .p(zpx(16.))
                .rounded(zpx(6.))
                .bg(t.md_canvas_subtle)
                .font_family(mono_font())
                .text_size(zpx(10.2))
                .line_height(zpx(10.2 * 1.45))
                .whitespace_nowrap()
                .text_align(TextAlign::Left)
                .child(SharedString::from(code.clone()))
                .into_any_element(),
            Block::BlockQuote(inner) => div()
                .px(zpx(12.))
                .border_l(zpx(3.))
                .border_color(t.md_border_default)
                .text_color(t.md_fg_muted)
                .child(self.blocks(inner, list_depth))
                .into_any_element(),
            Block::Alert { kind, blocks } => {
                let (border, fg, icon) = alert_style(*kind, t.is_dark());
                // `.markdown-alert { padding: .5rem 1rem; border-left: .25em }`
                div()
                    .py(zpx(6.))
                    .px(zpx(12.))
                    .border_l(zpx(3.))
                    .border_color(border)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(zpx(8.))
                            .text_color(fg)
                            .font_weight(FontWeight::MEDIUM)
                            .child(octicon(icon, fg))
                            .child(kind.title()),
                    )
                    .child(div().mt(BLOCK_GAP()).child(self.blocks(blocks, list_depth)))
                    .into_any_element()
            }
            Block::Rule => div()
                .h(zpx(3.))
                .w_full()
                .bg(t.md_border_default)
                .into_any_element(),
            Block::List { start, items } => self.list(*start, items, list_depth, false),
            Block::Table { align, head, rows } => self.table(align, head, rows),
            Block::Details {
                open,
                summary,
                blocks,
            } => self.details(*open, summary, blocks, list_depth),
            Block::Aligned { align, blocks } => {
                let outer = self.align.replace(Some(*align));
                let inner = self.blocks(blocks, list_depth);
                self.align.set(outer);
                match align {
                    Align::Center => inner.items_center().text_center(),
                    Align::Right => inner.items_end().text_right(),
                    Align::Left => inner,
                }
                .into_any_element()
            }
            Block::Footnotes(notes) => {
                // `section.footnotes { font-size: 12px; color: fg-muted;
                // border-top: 1px solid border-default }`
                let items: Vec<ListItem> = notes
                    .iter()
                    .map(|blocks| ListItem {
                        task: None,
                        blocks: blocks.clone(),
                    })
                    .collect();
                div()
                    .pt(zpx(12.))
                    .border_t_1()
                    .border_color(t.md_border_default)
                    .text_color(t.md_fg_muted)
                    .text_size(zpx(10.5))
                    .line_height(zpx(16.))
                    .child(self.list(Some(1), &items, list_depth, true))
                    .into_any_element()
            }
        }
    }

    fn list(
        &self,
        start: Option<u64>,
        items: &[ListItem],
        list_depth: usize,
        footnotes: bool,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().gap(ITEM_GAP());
        for (ix, item) in items.iter().enumerate() {
            let marker: AnyElement = match (item.task, start) {
                // `.task-list-item-checkbox { margin: 0 .2em .25em -1.4em }`
                (Some(checked), _) => div()
                    .h(LINE_HEIGHT())
                    .flex()
                    .items_center()
                    .child(crate::widgets::checkbox(
                        self.element_id(),
                        checked,
                        true,
                        self.cx,
                    ))
                    .into_any_element(),
                (None, Some(n)) => {
                    SharedString::from(format!("{}.", n + ix as u64)).into_any_element()
                }
                // disc, circle, square by nesting level
                (None, None) => SharedString::from(match list_depth {
                    0 => "•",
                    1 => "◦",
                    _ => "▪",
                })
                .into_any_element(),
            };
            col =
                col.child(
                    div()
                        .flex()
                        .flex_row()
                        .text_align(TextAlign::Left)
                        .child(
                            div()
                                .flex_none()
                                .w(LIST_INDENT())
                                .pr(zpx(if item.task.is_some() { 4. } else { 6. }))
                                .flex()
                                .justify_end()
                                .child(marker),
                        )
                        .child(div().flex_1().min_w_0().child(
                            self.blocks(&item.blocks, list_depth + usize::from(!footnotes)),
                        )),
                );
        }
        col.into_any_element()
    }

    /// GitHub's table: `display: block; width: max-content; max-width:
    /// 100%; overflow: auto`, collapsed 1 px borders, `th` bold and
    /// centred unless aligned, every second body row on the subtle canvas.
    fn table(
        &self,
        align: &[Option<Align>],
        head: &[RichText],
        rows: &[Vec<RichText>],
    ) -> AnyElement {
        let t = self.t;
        let columns = align.len().max(1);
        let cell = |text: &RichText, column: usize, row: usize, is_head: bool| {
            let align = align.get(column).copied().flatten();
            let zebra = !is_head && row % 2 == 1;
            div()
                .min_w_0()
                .px(zpx(13.))
                .py(zpx(6.))
                .border_r_1()
                .border_b_1()
                .border_color(t.md_border_default)
                .when(column == 0, |d| d.border_l_1())
                .when(is_head || (head.is_empty() && row == 0), |d| d.border_t_1())
                .when(zebra, |d| d.bg(t.md_canvas_subtle))
                .when(is_head, |d| d.font_weight(FontWeight::SEMIBOLD))
                .map(|d| match (align, is_head) {
                    (Some(Align::Center), _) | (None, true) => d.text_center(),
                    (Some(Align::Right), _) => d.text_right(),
                    _ => d.text_left(),
                })
                .child(self.paragraph(text))
        };
        let mut grid = div().grid().grid_cols_max_content(columns as u16);
        for (column, text) in head.iter().enumerate() {
            grid = grid.child(cell(text, column, 0, true));
        }
        for (row, cells) in rows.iter().enumerate() {
            for (column, text) in cells.iter().enumerate() {
                grid = grid.child(cell(text, column, row, false));
            }
        }
        div()
            .id(self.element_id())
            .max_w_full()
            .overflow_x_scroll()
            .flex()
            .flex_row()
            .child(grid)
            .into_any_element()
    }

    /// `<details>`: the summary behind a disclosure triangle, the rest
    /// shown while open.
    fn details(
        &self,
        initially_open: bool,
        summary: &RichText,
        blocks: &[Block],
        list_depth: usize,
    ) -> AnyElement {
        let n = self.next_details.get();
        self.next_details.set(n + 1);
        let key = SharedString::from(format!("{}-details-{n}", self.id));
        let toggled = self
            .cx
            .try_global::<DetailsToggles>()
            .is_some_and(|d| d.0.contains(&key));
        let open = initially_open != toggled;
        let t = self.t;
        let summary_row = div()
            .id(ElementId::Name(key.clone()))
            .flex()
            .flex_row()
            .items_start()
            .cursor_pointer()
            .text_align(TextAlign::Left)
            .on_click(move |_, window, cx| {
                let toggles = &mut cx.default_global::<DetailsToggles>().0;
                if !toggles.remove(&key) {
                    toggles.insert(key.clone());
                }
                window.refresh();
            })
            .child(
                div()
                    .flex_none()
                    .h(LINE_HEIGHT())
                    .w(zpx(14.))
                    .flex()
                    .items_center()
                    .child(
                        octicon(
                            if open {
                                Octicon::TriangleDown
                            } else {
                                Octicon::TriangleRight
                            },
                            t.text,
                        )
                        .size(zpx(12.)),
                    ),
            )
            .child(div().flex_1().min_w_0().child(self.paragraph(summary)));
        div()
            .flex()
            .flex_col()
            .child(summary_row)
            .when(open && !blocks.is_empty(), |d| {
                d.child(div().mt(zpx(4.)).child(self.blocks(blocks, list_depth)))
            })
            .into_any_element()
    }

    /// A fenced code block highlighted by its language.
    fn highlighted_code(&self, code: &str, language: Option<&str>) -> AnyElement {
        let t = self.t;
        let lines: Vec<&str> = code.split('\n').collect();
        let path = format!("code.{}", extension_for(language.unwrap_or("")));
        let mut highlights = Vec::new();
        if lines.len() <= 4_000
            && let Some(spans) = corvene_highlight::highlight_lines(&path, lines.iter().copied())
        {
            let mut offset = 0;
            for (line, spans) in lines.iter().zip(spans) {
                for span in spans {
                    let range = offset + span.range.start..offset + span.range.end;
                    if range.end <= code.len()
                        && code.is_char_boundary(range.start)
                        && code.is_char_boundary(range.end)
                    {
                        highlights.push((
                            range,
                            HighlightStyle {
                                color: Some(crate::diff_view_rows::token_color(span.class, t)),
                                ..HighlightStyle::default()
                            },
                        ));
                    }
                }
                offset += line.len() + 1;
            }
        }
        div()
            .id(self.element_id())
            .w_full()
            .overflow_x_scroll()
            .p(zpx(12.))
            .rounded(zpx(6.))
            .bg(t.md_canvas_subtle)
            .font_family(mono_font())
            .text_size(zpx(self.font_size * 0.85))
            .line_height(zpx(self.font_size * 0.85 * 1.45))
            .whitespace_nowrap()
            .text_align(TextAlign::Left)
            .child(
                StyledText::new(SharedString::from(code.to_string())).with_highlights(highlights),
            )
            .into_any_element()
    }

    /// A paragraph: one text when it has no images or formulas, else its
    /// lines of text runs, images and formulas.
    fn paragraph(&self, text: &RichText) -> AnyElement {
        if text.images.is_empty() && text.maths.is_empty() {
            return self.rich(text);
        }
        let justify = self.align.get();
        let mut col = div().flex().flex_col().min_w_0();
        for line in lines_of(text) {
            if line.is_empty() {
                col = col.child(div().h(LINE_HEIGHT()));
                continue;
            }
            let row = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_end()
                .min_w_0()
                .map(|d| match justify {
                    Some(Align::Center) => d.justify_center(),
                    Some(Align::Right) => d.justify_end(),
                    _ => d,
                })
                .children(line.into_iter().map(|piece| match piece {
                    Piece::Text(range) => {
                        let marks = self.slice_marks(range.clone());
                        div()
                            .min_w_0()
                            .flex_shrink(1.)
                            .child(self.rich_marked(&text.slice(range), &marks))
                            .into_any_element()
                    }
                    Piece::Image(ix) => self.image(&text.images[ix]),
                    Piece::Math(ix) => self.math(&text.maths[ix]),
                }));
            col = col.child(row);
        }
        col.into_any_element()
    }

    /// An inline formula, or its source when math is off.
    fn math(&self, math: &corvene_core::markdown::InlineMath) -> AnyElement {
        match &self.macros {
            Some(macros) => crate::math::inline(
                self.element_id(),
                &math.tex,
                math.display,
                macros,
                zpx(self.font_size),
                self.cx,
            ),
            None => div()
                .flex_none()
                .font_family(mono_font())
                .child(SharedString::from(if math.display {
                    format!("$${}$$", math.tex)
                } else {
                    format!("${}$", math.tex)
                }))
                .into_any_element(),
        }
    }

    /// The marks within `range` of the paragraph, from its start.
    fn slice_marks(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let Some((marks, _)) = &self.marks else {
            return Vec::new();
        };
        marks
            .iter()
            .filter(|m| m.end > range.start && m.start < range.end)
            .map(|m| m.start.max(range.start) - range.start..m.end.min(range.end) - range.start)
            .collect()
    }

    /// An image: the downloaded file at its size (at most the width there
    /// is), else its alt text.
    fn image(&self, image: &InlineImage) -> AnyElement {
        let t = self.t;
        if let Some(resolved) = self.images.as_ref().and_then(|r| r(&image.src)) {
            let element = img(resolved).max_w_full().object_fit(ObjectFit::ScaleDown);
            return match (image.width, image.height) {
                (Some(w), _) => element.w(zpx(w as f32)).into_any_element(),
                (None, Some(h)) => element.h(zpx(h as f32)).into_any_element(),
                _ => element.max_h(zpx(480.)).into_any_element(),
            };
        }
        let Some(url) = resolve_link(&image.src, self.base.as_deref()) else {
            return self.image_alt(image, false);
        };
        let entry = corvene_core::AppState::try_global(self.cx)
            .and_then(|s| s.read(self.cx).markdown_images.get(&url).cloned());
        let requested = entry.is_some();
        let entry = entry.unwrap_or_default();
        let size = image_box(image, entry.size);
        let link = image
            .link
            .as_deref()
            .and_then(|l| resolve_link(l, self.base.as_deref()));
        let element = match (&entry.path, size) {
            (Some(path), Some((w, h))) => div()
                .flex_none()
                .relative()
                .w(zpx(w))
                .max_w_full()
                .aspect_ratio(w / h.max(1.))
                .bg(t.background)
                .child(
                    img(path.clone())
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .with_fallback({
                            let alt = SharedString::from(image.alt.clone());
                            move || div().child(alt.clone()).into_any_element()
                        }),
                )
                .into_any_element(),
            (Some(path), None) => img(path.clone()).max_w_full().into_any_element(),
            (None, Some((w, h))) if !entry.failed => div()
                .flex_none()
                .w(zpx(w))
                .max_w_full()
                .aspect_ratio(w / h.max(1.))
                .bg(t.md_canvas_subtle)
                .into_any_element(),
            (None, _) => self.image_alt(image, entry.failed),
        };
        let element = if requested {
            element
        } else {
            // ask for it once it is laid out (a render has no `&mut App`)
            div()
                .flex_none()
                .max_w_full()
                .child(element)
                .child(canvas(
                    move |_, _, cx| {
                        cx.defer(move |cx| Dispatcher::request_markdown_image(&url, cx))
                    },
                    |_, _, _, _| {},
                ))
                .into_any_element()
        };
        match link {
            Some(link) => div()
                .id(self.element_id())
                .flex_none()
                .max_w_full()
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    crate::link_menu::open_link(link.to_string(), window, cx)
                })
                .child(element)
                .into_any_element(),
            None => element,
        }
    }

    /// An image's alt text in place of it (loading, or unreadable).
    fn image_alt(&self, image: &InlineImage, failed: bool) -> AnyElement {
        let t = self.t;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(zpx(4.))
            .text_color(t.md_fg_muted)
            .child(octicon(Octicon::Image, t.md_fg_muted))
            .when(!image.alt.is_empty() || failed, |d| {
                d.child(if image.alt.is_empty() {
                    SharedString::from("Image")
                } else {
                    SharedString::from(image.alt.clone())
                })
            })
            .into_any_element()
    }

    /// One paragraph: styled runs, clickable links (and the changed-word
    /// marks of a lone paragraph).
    fn rich(&self, text: &RichText) -> AnyElement {
        let marks = self.slice_marks(0..text.text.len());
        self.rich_marked(text, &marks)
    }

    fn rich_marked(&self, text: &RichText, marks: &[Range<usize>]) -> AnyElement {
        let t = self.t;
        let mut highlights = Vec::new();
        let mut fonts: Vec<(std::ops::Range<usize>, SharedString)> = Vec::new();
        let mut ranges = Vec::new();
        let mut urls: Vec<Option<String>> = Vec::new();
        let mut titles: Vec<Option<SharedString>> = Vec::new();
        let mut numbers: Vec<Option<u64>> = Vec::new();
        for span in &text.spans {
            let s = span.style;
            let link = span.link.is_some();
            highlights.push((
                span.range.clone(),
                HighlightStyle {
                    color: link.then_some(if self.link_buttons {
                        t.link
                    } else {
                        t.md_accent_fg
                    }),
                    font_weight: s.bold.then_some(FontWeight::BOLD),
                    font_style: s.italic.then_some(FontStyle::Italic),
                    background_color: s.code.then_some(t.md_neutral_muted),
                    underline: (link && self.underline).then_some(UnderlineStyle {
                        thickness: zpx(1.),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: s.strikethrough.then_some(StrikethroughStyle {
                        thickness: zpx(1.),
                        color: None,
                    }),
                    fade_out: None,
                },
            ));
            if s.code {
                fonts.push((span.range.clone(), mono_font().into()));
            }
            if let Some(href) = &span.link {
                ranges.push(span.range.clone());
                urls.push(resolve_link(href, self.base.as_deref()));
                titles.push(
                    (self.link_buttons && text.text[span.range.clone()] != **href)
                        .then(|| SharedString::from(href.clone())),
                );
                numbers.push(
                    self.issues
                        .as_ref()
                        .and_then(|gh| issue_number(&text.text[span.range.clone()], href, gh)),
                );
            }
        }
        if !marks.is_empty()
            && let Some((_, mark_bg)) = &self.marks
        {
            highlights = overlay_marks(&text.text, highlights, marks, *mark_bg);
        }
        let styled = StyledText::new(SharedString::from(text.text.clone()))
            .with_highlights(highlights)
            .with_font_family_overrides(fonts);
        if ranges.is_empty() {
            return styled.into_any_element();
        }
        let tips: Vec<(std::ops::Range<usize>, SharedString, Option<u64>)> = ranges
            .iter()
            .zip(titles)
            .zip(numbers)
            .filter_map(|((r, title), number)| Some((r.clone(), title?, number)))
            .collect();
        let issues = self.issues.clone();
        let text = InteractiveText::new(self.element_id(), styled).on_click(
            ranges,
            move |ix, window, cx| {
                if let Some(Some(url)) = urls.get(ix) {
                    crate::link_menu::open_link(url.to_string(), window, cx);
                }
            },
        );
        if tips.is_empty() {
            return text.into_any_element();
        }
        text.tooltip(move |ix, window, cx| {
            let (_, url, number) = tips.iter().find(|(r, _, _)| r.contains(&ix))?;
            let title = issues
                .as_ref()
                .zip(*number)
                .and_then(|(gh, number)| issue_title(gh, number, cx))
                .map_or_else(|| url.clone(), SharedString::from);
            Some(crate::widgets::tooltip(title)(window, cx))
        })
        .into_any_element()
    }
}

/// `highlights` (sorted, not overlapping) with `marks` laid over them as a
/// background: the ranges are cut where either starts or ends.
fn overlay_marks(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    marks: &[Range<usize>],
    bg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = highlights
        .iter()
        .flat_map(|(r, _)| [r.start, r.end])
        .chain(marks.iter().flat_map(|m| [m.start, m.end]))
        .map(|c| c.min(text.len()))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        let base = highlights
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, h)| *h);
        let marked = marks.iter().any(|m| m.start <= a && b <= m.end);
        match (base, marked) {
            (Some(mut h), true) => {
                h.background_color = Some(bg);
                out.push((a..b, h));
            }
            (Some(h), false) => out.push((a..b, h)),
            (None, true) => out.push((
                a..b,
                HighlightStyle {
                    background_color: Some(bg),
                    ..HighlightStyle::default()
                },
            )),
            (None, false) => {}
        }
    }
    out
}

/// A file extension the highlighter knows for a code fence's language.
pub fn extension_for(language: &str) -> String {
    let lang = language
        .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match lang.as_str() {
        "python" | "py" | "python3" | "ipython" | "ipython3" => "py",
        "javascript" | "js" | "node" => "js",
        "typescript" | "ts" => "ts",
        "rust" | "rs" => "rs",
        "ruby" | "rb" => "rb",
        "julia" | "jl" => "jl",
        "r" => "r",
        "bash" | "sh" | "shell" | "zsh" | "console" => "sh",
        "c++" | "cpp" | "cxx" => "cpp",
        "c" => "c",
        "csharp" | "c#" | "cs" => "cs",
        "fsharp" | "f#" => "fs",
        "golang" | "go" => "go",
        "java" => "java",
        "kotlin" | "kt" => "kt",
        "scala" => "scala",
        "haskell" | "hs" => "hs",
        "matlab" | "octave" => "m",
        "latex" | "tex" => "tex",
        "markdown" | "md" => "md",
        "yaml" | "yml" => "yml",
        "json" => "json",
        "toml" => "toml",
        "html" => "html",
        "css" => "css",
        "sql" => "sql",
        "powershell" | "ps1" => "ps1",
        "perl" | "pl" => "pl",
        "lua" => "lua",
        "" => "txt",
        other => return other.to_string(),
    }
    .to_string()
}
