//! Markdown as GPUI text - GHD `ui/lib/sandboxed-markdown.tsx` with
//! `static/common/markdown.css` (Primer's `.markdown-body`).
//!
//! GHD renders `marked` output inside a sandboxed iframe; Corvene parses with
//! `corvene_core::markdown` and lays the blocks out natively: headings (h1/h2
//! with the muted bottom border), paragraphs, bold / italic / strikethrough,
//! inline code (mono font on `--md-neutral-muted-color`), fenced code blocks
//! (`pre`: 16 px padding, 85 % mono, horizontal scroll), links in
//! `--md-accent-fg-color` that open in the browser, bullet / numbered lists and
//! block quotes. Vertical rhythm follows the CSS margins (16 px between
//! blocks, 24 px above headings and around rules). Unsupported constructs
//! arrive as plain text (see the core module).
//!
//! [`rich_text`] is GHD `ui/lib/rich-text.tsx` over a commit message: links
//! in `--link-button-color`, with the URL as the tooltip when they show
//! `#123` or `@name`.
//!
//! Deviations: text is not selectable (GPUI static text), link hover has no
//! colour change, and inline code keeps the paragraph's font size (GHD 85 %).
//!
//! Deviation (`881-issue-title-tooltips`): with
//! [`rich_text_with_issue_titles`] a `#123` link's tooltip is "#123 <title>"
//! from the repository's issue cache (hovering loads the cache), else the URL.

use std::rc::Rc;

use corvene_core::markdown::{Block, RichText, resolve_link};
use corvene_core::{Dispatcher, GitHubRepository};
use gpui_kit::prelude::*;
use gpui_kit::*;

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

/// Render `blocks` at `font_size` (GHD `--font-size`, 12 px) with
/// `line-height: 1.5`. `base_href` resolves relative links (GHD `baseHref`).
pub fn markdown(
    id: impl Into<SharedString>,
    blocks: &[Block],
    base_href: Option<&str>,
    cx: &App,
) -> Div {
    let underline =
        corvene_core::AppState::try_global(cx).is_some_and(|s| s.read(cx).settings.underline_links);
    let r = Renderer {
        id: id.into(),
        base: base_href.map(Rc::from),
        t: cx.ghd(),
        underline,
        link_buttons: false,
        issues: None,
        next: std::cell::Cell::new(0),
    };
    div()
        .text_size(zpx(12.))
        .line_height(zpx(18.))
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
        underline,
        link_buttons: true,
        issues: issues.map(Rc::new),
        next: std::cell::Cell::new(0),
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

struct Renderer<'a> {
    id: SharedString,
    base: Option<Rc<str>>,
    t: &'a GhdTheme,
    underline: bool,
    /// GHD `RichText`'s `LinkButton`s: `--link-button-color`, and the URL as
    /// the title when the text differs (`#123`, `@name`).
    link_buttons: bool,
    /// `881-issue-title-tooltips`: whose issue titles `#123` links show.
    issues: Option<Rc<GitHubRepository>>,
    next: std::cell::Cell<usize>,
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
            Block::Paragraph(text) => self.rich(text).into_any_element(),
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
                    .child(self.rich(text))
                    .into_any_element()
            }
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
                .child(SharedString::from(code.clone()))
                .into_any_element(),
            Block::BlockQuote(inner) => div()
                .px(zpx(12.))
                .border_l(zpx(3.))
                .border_color(t.md_border_default)
                .text_color(t.md_fg_muted)
                .child(self.blocks(inner, list_depth))
                .into_any_element(),
            Block::Rule => div()
                .h(zpx(3.))
                .w_full()
                .bg(t.md_border_default)
                .into_any_element(),
            Block::List { start, items } => {
                let mut col = div().flex().flex_col().gap(ITEM_GAP());
                for (ix, item) in items.iter().enumerate() {
                    let marker: SharedString = match start {
                        Some(n) => format!("{}.", n + ix as u64).into(),
                        // disc, circle, square by nesting level
                        None => match list_depth {
                            0 => "•",
                            1 => "◦",
                            _ => "▪",
                        }
                        .into(),
                    };
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .child(
                                div()
                                    .flex_none()
                                    .w(LIST_INDENT())
                                    .pr(zpx(6.))
                                    .flex()
                                    .justify_end()
                                    .child(marker),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.blocks(item, list_depth + 1)),
                            ),
                    );
                }
                col.into_any_element()
            }
        }
    }

    /// One paragraph: styled runs, clickable links.
    fn rich(&self, text: &RichText) -> AnyElement {
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
