//! Markdown → block model for `corvene_ui::markdown`, standing in for GHD
//! `ui/lib/sandboxed-markdown.tsx`, which renders `marked(markdown, { gfm:
//! true, breaks: true })` (sanitised by DOMPurify) inside a sandboxed iframe
//! styled by `static/common/markdown.css`.
//!
//! The walk over `pulldown-cmark` events keeps what GitHub's Markdown
//! shows: headings, paragraphs, bold / italic / strikethrough, inline code,
//! fenced and indented code, links, bullet, numbered and task lists, block
//! quotes and their `[!NOTE]` alert forms, rules, tables (with the column
//! alignment of the `:---:` row), footnotes and images (inline, with their
//! link and size).
//!
//! Raw HTML is read the way DOMPurify leaves it for GitHub's own pages:
//! `<details>` / `<summary>` become a collapsible block (nested too),
//! `<p align>` / `<div align>` / `<h1 align>` an aligned block, `<img>`
//! (also inside `<picture>`) an image with its `width` / `height`, `<br>` a
//! line break, `<b>` / `<strong>`, `<i>` / `<em>`, `<s>` / `<del>`,
//! `<code>` / `<kbd>` and `<a href>` their inline style, `<ul>` / `<ol>` /
//! `<li>`, `<blockquote>`, `<pre>`, `<hr>` and simple `<table>`s their
//! blocks, and comments disappear. Other tags are dropped and keep their
//! text (`<sub>` / `<sup>` have no smaller type).
//!
//! As with `breaks: true`, a single newline inside a paragraph is a line
//! break. Bare `http(s)://` URLs are linkified as GFM does. GHD's markdown
//! filters (emoji, `@mention`, `#123` issue and commit-SHA links, videos) are
//! not ported.

use std::ops::Range;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::text_tokens::{Token, TokenOptions, TokenRepository, tokenize_with};

/// Inline styles active over a span of text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub code: bool,
}

/// A styled run of a [`RichText`]. Spans are sorted, non-overlapping, and
/// only exist for text that is styled or linked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: InlineStyle,
    pub link: Option<String>,
}

/// An image inside a paragraph (`![alt](src)`, `<img>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineImage {
    /// Byte offset into the paragraph's text where the image sits.
    pub at: usize,
    /// As written; relative sources resolve like links ([`resolve_link`]).
    pub src: String,
    pub alt: String,
    /// The `width` / `height` attributes in CSS pixels, when given in
    /// pixels.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The link around the image (a badge), when there is one.
    pub link: Option<String>,
}

/// TeX math inside a paragraph (`$…$`; `$$…$$` amid text), when parsed
/// with [`ParseOptions::math`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineMath {
    /// Byte offset into the paragraph's text where the formula sits.
    pub at: usize,
    pub tex: String,
    /// `$$…$$`: display style, on a line of its own.
    pub display: bool,
}

/// One paragraph's text with its styled spans, images and formulas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub spans: Vec<Span>,
    /// Sorted by `at`.
    pub images: Vec<InlineImage>,
    /// Sorted by `at`.
    pub maths: Vec<InlineMath>,
}

impl RichText {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images.is_empty() && self.maths.is_empty()
    }

    /// The linked spans in order, for click handling.
    pub fn links(&self) -> impl Iterator<Item = (&Range<usize>, &str)> {
        self.spans
            .iter()
            .filter_map(|s| s.link.as_deref().map(|l| (&s.range, l)))
    }

    fn push(&mut self, text: &str, style: InlineStyle, link: Option<&str>) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        if style == InlineStyle::default() && link.is_none() {
            return;
        }
        if let Some(last) = self.spans.last_mut()
            && last.range.end == start
            && last.style == style
            && last.link.as_deref() == link
        {
            last.range.end = end;
            return;
        }
        self.spans.push(Span {
            range: start..end,
            style,
            link: link.map(str::to_string),
        });
    }

    /// The part of the text from `range` with its spans (offsets from the
    /// start of the range), for drawing the text between images.
    pub fn slice(&self, range: Range<usize>) -> RichText {
        let spans = self
            .spans
            .iter()
            .filter(|s| s.range.end > range.start && s.range.start < range.end)
            .map(|s| Span {
                range: s.range.start.max(range.start) - range.start
                    ..s.range.end.min(range.end) - range.start,
                style: s.style,
                link: s.link.clone(),
            })
            .collect();
        RichText {
            text: self.text[range].to_string(),
            spans,
            images: Vec::new(),
            maths: Vec::new(),
        }
    }

    /// Trailing line breaks left by a soft break before a block end.
    fn trim_end(&mut self) {
        self.trim_end_matches(&['\n', ' ']);
    }

    fn trim_end_matches(&mut self, chars: &[char]) {
        // white space before the last image or formula stays
        let keep = self
            .images
            .last()
            .map_or(0, |i| i.at)
            .max(self.maths.last().map_or(0, |m| m.at));
        let trimmed = self.text.trim_end_matches(chars).len().max(keep);
        self.text.truncate(trimmed);
        self.spans.retain(|s| s.range.start < trimmed);
        if let Some(last) = self.spans.last_mut() {
            last.range.end = last.range.end.min(trimmed);
        }
        for image in &mut self.images {
            image.at = image.at.min(trimmed);
        }
        for math in &mut self.maths {
            math.at = math.at.min(trimmed);
        }
    }

    /// Whether anything but white space was collected.
    fn has_content(&self) -> bool {
        !self.images.is_empty() || !self.maths.is_empty() || !self.text.trim().is_empty()
    }

    /// The paragraph is one `$$…$$` formula and nothing else.
    fn lone_display_math(&self) -> Option<&str> {
        match self.maths.as_slice() {
            [m] if m.display && self.images.is_empty() && self.text.trim().is_empty() => {
                Some(&m.tex)
            }
            _ => None,
        }
    }
}

/// A table column's or an HTML block's alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    fn from_attr(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Some(Self::Left),
            "center" | "middle" => Some(Self::Center),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// GitHub's alert block quotes (`> [!NOTE]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }
}

/// One list item; `task` is a task list item's checkbox.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListItem {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

/// A block of a Markdown document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// `level` is 1…6.
    Heading {
        level: u8,
        text: RichText,
    },
    Paragraph(RichText),
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    /// `start` is the first number of an ordered list, `None` for bullets.
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    BlockQuote(Vec<Block>),
    Alert {
        kind: AlertKind,
        blocks: Vec<Block>,
    },
    Rule,
    /// `align` has one entry per column (`None`: the default, left).
    Table {
        align: Vec<Option<Align>>,
        head: Vec<RichText>,
        rows: Vec<Vec<RichText>>,
    },
    /// `<details>`: `open` is the attribute (shown expanded at first).
    Details {
        open: bool,
        summary: RichText,
        blocks: Vec<Block>,
    },
    /// `<p align>`, `<div align>`.
    Aligned {
        align: Align,
        blocks: Vec<Block>,
    },
    /// The footnote definitions, numbered from 1 in reference order.
    Footnotes(Vec<Vec<Block>>),
    /// A display formula (`$$…$$` alone in a paragraph, a ```` ```math ````
    /// fence, a LaTeX math environment) when parsed with
    /// [`ParseOptions::math`]. `number` is a LaTeX equation number.
    Math {
        tex: String,
        number: Option<String>,
    },
}

/// How [`parse_with`] reads Markdown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseOptions {
    /// `$…$` / `$$…$$` / ```` ```math ```` and bare `\begin{align}`-style
    /// environments become formulas, as github.com and Jupyter show them
    /// (Corvene `1331-notebook-diff`, `1333-markdown-math`). Off, they stay
    /// text, as in GHD.
    pub math: bool,
}

/// Parse Markdown into blocks.
pub fn parse(markdown: &str) -> Vec<Block> {
    parse_with(markdown, ParseOptions::default())
}

/// Parse Markdown into blocks with `options`.
pub fn parse_with(markdown: &str, options: ParseOptions) -> Vec<Block> {
    let mut flags = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    let wrapped;
    let markdown = if options.math {
        flags |= Options::ENABLE_MATH;
        wrapped = wrap_math_environments(markdown);
        wrapped.as_str()
    } else {
        markdown
    };
    let mut walk = Walk {
        math: options.math,
        ..Walk::default()
    };
    for event in Parser::new_ext(markdown, flags) {
        walk.event(event);
    }
    walk.finish()
}

/// Math environments MathJax typesets where they stand in Jupyter markdown.
const MATH_ENVIRONMENTS: [&str; 16] = [
    "equation",
    "equation*",
    "align",
    "align*",
    "aligned",
    "gather",
    "gather*",
    "gathered",
    "multline",
    "multline*",
    "split",
    "eqnarray",
    "eqnarray*",
    "alignat",
    "alignat*",
    "CD",
];

/// Puts `$$` lines around a `\begin{align}` … `\end{align}` that starts a
/// line outside code (Jupyter's MathJax renders those without dollars).
fn wrap_math_environments(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len() + 16);
    let mut open: Option<String> = None;
    let mut fence: Option<&str> = None;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if open.is_none() {
            for marker in ["```", "~~~"] {
                if trimmed.starts_with(marker) {
                    fence = match fence {
                        Some(m) if m == marker => None,
                        None => Some(marker),
                        other => other,
                    };
                }
            }
        }
        if fence.is_none()
            && open.is_none()
            && let Some(env) = trimmed
                .strip_prefix("\\begin{")
                .and_then(|r| r.split_once('}'))
                .map(|(env, _)| env)
                .filter(|env| MATH_ENVIRONMENTS.contains(env))
        {
            out.push_str("$$\n");
            open = Some(format!("\\end{{{env}}}"));
        }
        out.push_str(line);
        if let Some(end) = &open
            && line.contains(end.as_str())
        {
            if !line.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("$$\n");
            open = None;
        }
    }
    if open.is_some() {
        out.push_str("\n$$\n");
    }
    out
}

/// Plain text of a document (search, accessibility labels, previews).
pub fn plain_text(blocks: &[Block]) -> String {
    fn walk(blocks: &[Block], out: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Heading { text, .. } | Block::Paragraph(text) => out.push(text.text.clone()),
                Block::CodeBlock { code, .. } => out.push(code.clone()),
                Block::List { items, .. } => items.iter().for_each(|i| walk(&i.blocks, out)),
                Block::BlockQuote(inner)
                | Block::Alert { blocks: inner, .. }
                | Block::Aligned { blocks: inner, .. } => walk(inner, out),
                Block::Rule => {}
                Block::Table { head, rows, .. } => {
                    for row in std::iter::once(head).chain(rows) {
                        let cells: Vec<&str> = row.iter().map(|c| c.text.as_str()).collect();
                        out.push(cells.join(" "));
                    }
                }
                Block::Details {
                    summary, blocks, ..
                } => {
                    out.push(summary.text.clone());
                    walk(blocks, out);
                }
                Block::Footnotes(notes) => notes.iter().for_each(|n| walk(n, out)),
                Block::Math { tex, .. } => out.push(tex.clone()),
            }
        }
    }
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out.join("\n")
}

/// Every image source in `blocks`, in document order (prefetching).
pub fn image_sources(blocks: &[Block]) -> Vec<&str> {
    fn rich<'a>(text: &'a RichText, out: &mut Vec<&'a str>) {
        out.extend(text.images.iter().map(|i| i.src.as_str()));
    }
    fn walk<'a>(blocks: &'a [Block], out: &mut Vec<&'a str>) {
        for block in blocks {
            match block {
                Block::Heading { text, .. } | Block::Paragraph(text) => rich(text, out),
                Block::List { items, .. } => items.iter().for_each(|i| walk(&i.blocks, out)),
                Block::BlockQuote(inner)
                | Block::Alert { blocks: inner, .. }
                | Block::Aligned { blocks: inner, .. } => walk(inner, out),
                Block::Table { head, rows, .. } => head
                    .iter()
                    .chain(rows.iter().flatten())
                    .for_each(|c| rich(c, out)),
                Block::Details {
                    summary, blocks, ..
                } => {
                    rich(summary, out);
                    walk(blocks, out);
                }
                Block::Footnotes(notes) => notes.iter().for_each(|n| walk(n, out)),
                Block::CodeBlock { .. } | Block::Rule | Block::Math { .. } => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out
}

/// The URL a clicked link opens, as GHD's link interceptor does: only
/// `http(s)` links, relative ones resolved against `base_href` (the page the
/// Markdown came from, e.g. a pull request's `html_url`).
pub fn resolve_link(href: &str, base_href: Option<&str>) -> Option<String> {
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(href.to_string());
    }
    if lower.contains(':') && !lower.starts_with('/') && !lower.starts_with('#') {
        // mailto:, javascript:, file: …
        return None;
    }
    let base = base_href?;
    let scheme_end = base.find("://")? + 3;
    let origin_end = base[scheme_end..]
        .find('/')
        .map_or(base.len(), |ix| scheme_end + ix);
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("{}{rest}", &base[..scheme_end]));
    }
    if href.starts_with('/') {
        return Some(format!("{}{href}", &base[..origin_end]));
    }
    let base_no_fragment = base.split('#').next().unwrap_or(base);
    if href.starts_with('#') {
        return Some(format!("{base_no_fragment}{href}"));
    }
    let dir_end = base_no_fragment
        .rfind('/')
        .filter(|ix| *ix >= origin_end)
        .unwrap_or(base_no_fragment.len());
    Some(format!("{}/{href}", &base_no_fragment[..dir_end]))
}

enum Container {
    Root,
    Quote,
    Alert(AlertKind),
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        task: Option<bool>,
    },
    Details {
        open: bool,
        summary: Option<RichText>,
    },
    Aligned(Align),
    Footnote(String),
}

struct Frame {
    container: Container,
    blocks: Vec<Block>,
    /// Opened by an HTML tag (closed by its end tag, or by the Markdown
    /// block around it ending).
    html: bool,
}

/// Where the inline text being collected ends up.
enum InlineTarget {
    Paragraph,
    Heading(u8),
    Cell,
    Summary,
}

/// A table being read (Markdown or HTML).
struct TableBuild {
    align: Vec<Option<Align>>,
    head: Vec<RichText>,
    rows: Vec<Vec<RichText>>,
    in_head: bool,
    /// HTML: the current row's cells are all `<th>` so far.
    html_row_is_head: bool,
    html: bool,
}

/// An image whose alt text is being read (`![alt](src)`).
struct PendingImage {
    src: String,
    alt: String,
    link: Option<String>,
}

/// An HTML element that may have opened a frame or a heading.
struct HtmlOpen {
    tag: String,
    /// It opened an [`Container::Aligned`] frame.
    aligned: bool,
}

#[derive(Default)]
struct Walk {
    frames: Vec<Frame>,
    inline: Option<(InlineTarget, RichText)>,
    code: Option<(Option<String>, String)>,
    /// The code block comes from `<pre>` (ended by `</pre>`).
    code_html: bool,
    bold: u32,
    italic: u32,
    strikethrough: u32,
    /// `<code>` / `<kbd>` nesting.
    code_inline: u32,
    links: Vec<Option<String>>,
    tables: Vec<TableBuild>,
    image: Option<PendingImage>,
    /// Inside an HTML comment that spans events.
    in_comment: bool,
    html_open: Vec<HtmlOpen>,
    /// Footnote labels in the order of their first reference.
    footnote_order: Vec<String>,
    footnotes: Vec<(String, Vec<Block>)>,
    /// [`ParseOptions::math`].
    math: bool,
}

/// A commit message as GHD's `RichText` shows it: the
/// [`crate::text_tokens`] emoji, `#123` issues, `@mentions` and links.
///
/// With `extras` (flag `804`), as GitHub.com shows it: `code` spans in
/// backticks (on one line; the backticks are dropped), bare `http(s)` URLs
/// also after punctuation and without trailing punctuation, and with
/// `commit_base` (the repository's `html_url`) 7–40 character hex words that
/// mix letters and digits linked to `<commit_base>/commit/<sha>`.
///
/// `options` are the tokenizer's own deviations ([`TokenOptions`]).
pub fn commit_message_rich_text(
    text: &str,
    repository: Option<&TokenRepository>,
    options: TokenOptions,
    extras: bool,
    commit_base: Option<&str>,
) -> RichText {
    let mut out = RichText::default();
    if !extras {
        push_tokens(&mut out, text, repository, options, None);
        return out;
    }
    let code = InlineStyle {
        code: true,
        ..InlineStyle::default()
    };
    let mut rest = text;
    while !rest.is_empty() {
        let span = rest.find('`').and_then(|open| {
            let close = rest[open + 1..].find(['`', '\n'])?;
            (rest.as_bytes()[open + 1 + close] == b'`' && close > 0)
                .then_some((open, open + 1 + close))
        });
        let Some((open, close)) = span else {
            push_tokens(
                &mut out,
                rest,
                repository,
                options.clone(),
                Some(commit_base),
            );
            break;
        };
        push_tokens(
            &mut out,
            &rest[..open],
            repository,
            options.clone(),
            Some(commit_base),
        );
        out.push(&rest[open + 1..close], code, None);
        rest = &rest[close + 1..];
    }
    out
}

/// GHD `ExpandableCommitSummary`'s title and description
/// (`wrapRichTextCommitMessage`, [`crate::text_tokens::wrap_rich_text_commit_message`]):
/// a summary longer than 72 characters continues at the start of the
/// description. `extras` and `commit_base` as in [`commit_message_rich_text`];
/// once the summary wraps, `code` spans show their backticks (the wrap works
/// on GHD's tokens, which have no code spans). `options` as in
/// [`commit_message_rich_text`].
pub fn commit_summary_rich_text(
    summary: &str,
    body: &str,
    repository: Option<&TokenRepository>,
    options: TokenOptions,
    extras: bool,
    commit_base: Option<&str>,
) -> (RichText, RichText) {
    let (title, description) = crate::text_tokens::wrap_rich_text_commit_message_with(
        summary,
        body,
        repository,
        options.clone(),
    );
    if title == tokenize_with(summary.trim_end(), repository, options.clone()) {
        // nothing moved
        return (
            commit_message_rich_text(
                summary.trim_end(),
                repository,
                options.clone(),
                extras,
                commit_base,
            ),
            commit_message_rich_text(body.trim_end(), repository, options, extras, commit_base),
        );
    }
    let extras = extras.then_some(commit_base);
    let rich = |tokens: Vec<Token>| {
        let mut out = RichText::default();
        push_token_list(&mut out, tokens, extras);
        out
    };
    (rich(title), rich(description))
}

/// GHD's tokens for `text`; `extras` (with the SHA base) as in
/// [`commit_message_rich_text`].
fn push_tokens(
    out: &mut RichText,
    text: &str,
    repository: Option<&TokenRepository>,
    options: TokenOptions,
    extras: Option<Option<&str>>,
) {
    push_token_list(out, tokenize_with(text, repository, options), extras);
}

/// [`push_tokens`] of tokens already found.
fn push_token_list(out: &mut RichText, tokens: Vec<Token>, extras: Option<Option<&str>>) {
    let plain = InlineStyle::default();
    for token in tokens {
        match (&token, extras) {
            (Token::Text(text), Some(commit_base)) => push_autolinked(out, text, commit_base),
            (Token::Link { text, url }, Some(_)) if text == url => {
                let linked = url.trim_end_matches(URL_TRAILING_PUNCTUATION);
                out.push(linked, plain, Some(linked));
                out.push(&url[linked.len()..], plain, None);
            }
            (Token::Link { text, url }, _) => out.push(text, plain, Some(url)),
            _ => out.push(token.display(), plain, None),
        }
    }
}

const URL_TRAILING_PUNCTUATION: [char; 8] = ['.', ',', ':', ';', '!', '?', ')', '\''];

/// Plain text with its bare URLs and (with `commit_base`) SHAs linked.
fn push_autolinked(out: &mut RichText, text: &str, commit_base: Option<&str>) {
    let mut links: Vec<(Range<usize>, String)> = bare_urls(text)
        .into_iter()
        .map(|r| (r.clone(), text[r].to_string()))
        .collect();
    if let Some(base) = commit_base {
        let mut start = None;
        for (ix, c) in text
            .char_indices()
            .chain(std::iter::once((text.len(), ' ')))
        {
            if c.is_ascii_alphanumeric() {
                start.get_or_insert(ix);
                continue;
            }
            let Some(from) = start.take() else {
                continue;
            };
            let word = &text[from..ix];
            let is_sha = (7..=40).contains(&word.len())
                && word.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                && word.bytes().any(|b| b.is_ascii_digit())
                && word.bytes().any(|b| b.is_ascii_alphabetic());
            if is_sha && !links.iter().any(|(r, _)| r.contains(&from)) {
                links.push((from..ix, format!("{base}/commit/{word}")));
            }
        }
        links.sort_by_key(|(r, _)| r.start);
    }
    let mut at = 0;
    for (range, url) in links {
        out.push(&text[at..range.start], InlineStyle::default(), None);
        out.push(&text[range.clone()], InlineStyle::default(), Some(&url));
        at = range.end;
    }
    out.push(&text[at..], InlineStyle::default(), None);
}

/// Bare `http://` / `https://` URLs in `text` as byte ranges, without the
/// trailing punctuation GFM's extended autolinks leave out.
fn bare_urls(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(ix) = text[from..].find("http") {
        let start = from + ix;
        let rest = &text[start..];
        let scheme = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            from = start + 4;
            continue;
        };
        let len = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
            .unwrap_or(rest.len());
        let url = rest[..len].trim_end_matches(URL_TRAILING_PUNCTUATION);
        let preceded_by_word = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric());
        if url.len() > scheme && !preceded_by_word {
            out.push(start..start + url.len());
        }
        from = start + len.max(4);
    }
    out
}

/// One piece of raw HTML.
#[derive(Debug, PartialEq, Eq)]
enum HtmlToken<'a> {
    Text(&'a str),
    Tag {
        name: String,
        close: bool,
        attrs: Vec<(String, String)>,
    },
}

/// Split raw HTML into text and tags; `in_comment` carries a comment that
/// continues past `html` to the next piece.
fn html_tokens<'a>(html: &'a str, in_comment: &mut bool) -> Vec<HtmlToken<'a>> {
    let mut out = Vec::new();
    let mut rest = html;
    loop {
        if *in_comment {
            match rest.find("-->") {
                Some(end) => {
                    *in_comment = false;
                    rest = &rest[end + 3..];
                }
                None => return out,
            }
        }
        let Some(open) = rest.find('<') else {
            if !rest.is_empty() {
                out.push(HtmlToken::Text(rest));
            }
            return out;
        };
        if open > 0 {
            out.push(HtmlToken::Text(&rest[..open]));
        }
        let after = &rest[open..];
        if let Some(comment) = after.strip_prefix("<!--") {
            *in_comment = true;
            rest = comment;
            continue;
        }
        let Some((tag, len)) = parse_tag(after) else {
            // a lone `<`
            out.push(HtmlToken::Text(&after[..1]));
            rest = &after[1..];
            continue;
        };
        out.push(tag);
        rest = &after[len..];
    }
}

/// `<name attr="v" attr='v' attr=v attr>` or `</name>` at the start of `s`,
/// with its length.
fn parse_tag(s: &str) -> Option<(HtmlToken<'_>, usize)> {
    let bytes = s.as_bytes();
    let mut ix = 1;
    let close = bytes.get(ix) == Some(&b'/');
    if close {
        ix += 1;
    }
    let name_start = ix;
    while ix < bytes.len() && (bytes[ix].is_ascii_alphanumeric() || bytes[ix] == b'-') {
        ix += 1;
    }
    if ix == name_start || !bytes[name_start].is_ascii_alphabetic() {
        // `<!DOCTYPE>`, `<?xml?>`: dropped whole
        if matches!(bytes.get(1), Some(b'!' | b'?')) {
            let end = s.find('>')?;
            return Some((HtmlToken::Text(""), end + 1));
        }
        return None;
    }
    let name = s[name_start..ix].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while ix < bytes.len() && bytes[ix].is_ascii_whitespace() {
            ix += 1;
        }
        match bytes.get(ix) {
            None => return None,
            Some(b'>') => return Some((HtmlToken::Tag { name, close, attrs }, ix + 1)),
            Some(b'/') => {
                ix += 1;
                continue;
            }
            _ => {}
        }
        let key_start = ix;
        while ix < bytes.len()
            && !bytes[ix].is_ascii_whitespace()
            && !matches!(bytes[ix], b'=' | b'>' | b'/')
        {
            ix += 1;
        }
        let key = s[key_start..ix].to_ascii_lowercase();
        while ix < bytes.len() && bytes[ix].is_ascii_whitespace() {
            ix += 1;
        }
        let mut value = String::new();
        if bytes.get(ix) == Some(&b'=') {
            ix += 1;
            while ix < bytes.len() && bytes[ix].is_ascii_whitespace() {
                ix += 1;
            }
            match bytes.get(ix) {
                Some(&q @ (b'"' | b'\'')) => {
                    let end = s[ix + 1..].find(q as char)? + ix + 1;
                    value = decode_entities(&s[ix + 1..end]);
                    ix = end + 1;
                }
                _ => {
                    let start = ix;
                    while ix < bytes.len() && !bytes[ix].is_ascii_whitespace() && bytes[ix] != b'>'
                    {
                        ix += 1;
                    }
                    value = decode_entities(&s[start..ix]);
                }
            }
        }
        if key.is_empty() {
            ix += 1;
            continue;
        }
        attrs.push((key, value));
    }
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A `width` / `height` attribute in pixels (`300`, `300px`); percentages
/// and other units give `None`.
fn pixel_attr(attrs: &[(String, String)], key: &str) -> Option<u32> {
    let value = attr(attrs, key)?.trim();
    let value = value.strip_suffix("px").unwrap_or(value).trim();
    value
        .parse::<f32>()
        .ok()
        .filter(|v| *v > 0.)
        .map(|v| v.round() as u32)
}

/// The HTML character references Markdown authors use.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let end = after.find(';').filter(|end| {
            *end <= 10
                && after[..*end]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'#')
        });
        let decoded = end.and_then(|end| {
            let name = &after[..end];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                "copy" => Some('©'),
                "reg" => Some('®'),
                "trade" => Some('™'),
                "hellip" => Some('…'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "middot" => Some('·'),
                "times" => Some('×'),
                "rarr" => Some('→'),
                "larr" => Some('←'),
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((ch, end))
        });
        match decoded {
            Some((ch, end)) => {
                out.push(ch);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Runs of white space as one space, as HTML lays out text.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() && ch != '\u{a0}' {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(ch);
            space = false;
        }
    }
    out
}

impl Walk {
    fn frame(&mut self) -> &mut Frame {
        if self.frames.is_empty() {
            self.frames.push(Frame {
                container: Container::Root,
                blocks: Vec::new(),
                html: false,
            });
        }
        let last = self.frames.len() - 1;
        &mut self.frames[last]
    }

    fn style(&self) -> InlineStyle {
        InlineStyle {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strikethrough: self.strikethrough > 0,
            code: self.code_inline > 0,
        }
    }

    fn link(&self) -> Option<String> {
        self.links.last().cloned().flatten()
    }

    /// The paragraph that inline content goes into (an implicit one in a
    /// tight list item).
    fn inline(&mut self) -> &mut RichText {
        &mut self
            .inline
            .get_or_insert_with(|| (InlineTarget::Paragraph, RichText::default()))
            .1
    }

    fn flush_inline(&mut self) {
        let Some((target, mut text)) = self.inline.take() else {
            return;
        };
        text.trim_end();
        match target {
            InlineTarget::Heading(level) => {
                self.frame().blocks.push(Block::Heading { level, text })
            }
            InlineTarget::Paragraph if !text.has_content() => {}
            InlineTarget::Paragraph => match text.lone_display_math() {
                Some(tex) => {
                    let tex = tex.trim().to_string();
                    self.frame().blocks.push(Block::Math { tex, number: None })
                }
                None => self.frame().blocks.push(Block::Paragraph(text)),
            },
            InlineTarget::Cell => {
                if let Some(table) = self.tables.last_mut() {
                    if table.in_head {
                        table.head.push(text);
                    } else if let Some(row) = table.rows.last_mut() {
                        row.push(text);
                    } else {
                        table.rows.push(vec![text]);
                    }
                }
            }
            InlineTarget::Summary => {
                let details = self
                    .frames
                    .iter_mut()
                    .rev()
                    .find_map(|f| match &mut f.container {
                        Container::Details { summary, .. } => Some(summary),
                        _ => None,
                    });
                match details {
                    Some(summary) => *summary = Some(text),
                    None if text.has_content() => self.frame().blocks.push(Block::Paragraph(text)),
                    None => {}
                }
            }
        }
    }

    fn push_block(&mut self, block: Block) {
        self.flush_inline();
        self.frame().blocks.push(block);
    }

    fn push_frame(&mut self, container: Container, html: bool) {
        self.flush_inline();
        self.frame();
        self.frames.push(Frame {
            container,
            blocks: Vec::new(),
            html,
        });
    }

    /// Close the top frame into its parent.
    fn pop_one(&mut self) {
        self.flush_inline();
        if self.frames.len() < 2 {
            return;
        }
        let Some(frame) = self.frames.pop() else {
            return;
        };
        let blocks = frame.blocks;
        match frame.container {
            Container::Root => {}
            Container::Quote => self.frame().blocks.push(Block::BlockQuote(blocks)),
            Container::Alert(kind) => self.frame().blocks.push(Block::Alert { kind, blocks }),
            Container::List { start, items } => {
                self.frame().blocks.push(Block::List { start, items })
            }
            Container::Item { task } => {
                if let Container::List { items, .. } = &mut self.frame().container {
                    items.push(ListItem { task, blocks });
                } else {
                    self.frame().blocks.extend(blocks);
                }
            }
            Container::Details { open, summary } => self.frame().blocks.push(Block::Details {
                open,
                summary: summary.unwrap_or_else(|| {
                    let mut text = RichText::default();
                    text.push("Details", InlineStyle::default(), None);
                    text
                }),
                blocks,
            }),
            Container::Aligned(align) => {
                if !blocks.is_empty() {
                    self.frame().blocks.push(Block::Aligned { align, blocks })
                }
            }
            Container::Footnote(label) => self.footnotes.push((label, blocks)),
        }
    }

    /// A Markdown container ends: HTML frames left open inside it close
    /// with it.
    fn pop_frame(&mut self) {
        while self.frames.len() > 1 && self.frames.last().is_some_and(|f| f.html) {
            self.pop_one();
        }
        self.pop_one();
    }

    /// An HTML end tag: close the nearest frame it matches, when only HTML
    /// frames are open above it (a stray end tag is ignored).
    fn pop_html_frame(&mut self, matches: impl Fn(&Container) -> bool) {
        let Some(ix) = self
            .frames
            .iter()
            .rposition(|f| f.html && matches(&f.container))
        else {
            return;
        };
        if self.frames[ix + 1..].iter().any(|f| !f.html) {
            return;
        }
        while self.frames.len() > ix {
            self.pop_one();
        }
    }

    /// Text, with bare URLs outside links turned into links.
    fn text(&mut self, text: &str) {
        if let Some((_, code)) = &mut self.code {
            code.push_str(text);
            return;
        }
        if let Some(image) = &mut self.image {
            image.alt.push_str(text);
            return;
        }
        let style = self.style();
        if self.links.last().is_some() || style.code {
            let link = self.link();
            self.inline().push(text, style, link.as_deref());
            return;
        }
        let mut last = 0;
        for range in bare_urls(text) {
            let url = &text[range.clone()];
            self.inline().push(&text[last..range.start], style, None);
            self.inline().push(url, style, Some(url));
            last = range.end;
        }
        self.inline().push(&text[last..], style, None);
    }

    fn push_math(&mut self, tex: &str, display: bool) {
        if let Some(image) = &mut self.image {
            image.alt.push_str(tex);
            return;
        }
        let inline = self.inline();
        let at = inline.text.len();
        inline.maths.push(InlineMath {
            at,
            tex: tex.to_string(),
            display,
        });
    }

    fn push_image(&mut self, src: String, alt: String, width: Option<u32>, height: Option<u32>) {
        if src.trim().is_empty() {
            return;
        }
        let link = self.link();
        let inline = self.inline();
        let at = inline.text.len();
        inline.images.push(InlineImage {
            at,
            src,
            alt,
            width,
            height,
            link,
        });
    }

    /// Raw HTML: a block (`Event::Html`, one or more lines) or a tag inside
    /// a paragraph (`Event::InlineHtml`).
    fn html(&mut self, html: &str, block: bool) {
        let mut in_comment = self.in_comment;
        let tokens = html_tokens(html, &mut in_comment);
        self.in_comment = in_comment;
        for token in tokens {
            match token {
                HtmlToken::Text(text) => self.html_text(text, block),
                HtmlToken::Tag { name, close, attrs } => self.html_tag(&name, close, &attrs),
            }
        }
    }

    fn html_text(&mut self, text: &str, block: bool) {
        if text.is_empty() {
            return;
        }
        let decoded = decode_entities(text);
        if self.code.is_some() || self.image.is_some() {
            self.text(&decoded);
            return;
        }
        if !block {
            self.text(&decoded);
            return;
        }
        let collapsed = collapse_whitespace(&decoded);
        if collapsed.trim().is_empty() {
            // white space between tags only separates inline content
            let separates = self
                .inline
                .as_ref()
                .is_some_and(|(_, t)| t.has_content() && !t.text.ends_with([' ', '\n']));
            if separates {
                self.inline().push(" ", InlineStyle::default(), None);
            }
            return;
        }
        let at_line_start = self
            .inline
            .as_ref()
            .is_none_or(|(_, t)| t.text.is_empty() || t.text.ends_with([' ', '\n']));
        let text = if at_line_start {
            collapsed.trim_start()
        } else {
            collapsed.as_str()
        };
        self.text(text);
    }

    fn html_tag(&mut self, name: &str, close: bool, attrs: &[(String, String)]) {
        if self.code_html && self.code.is_some() {
            // `<pre>`: only its end matters
            if name == "pre" && close {
                self.end_code_block();
            }
            return;
        }
        match (name, close) {
            ("br", _) => {
                if let Some((_, code)) = &mut self.code {
                    code.push('\n');
                } else if self.image.is_none() {
                    let inline = self.inline();
                    inline.trim_end_matches(&[' ']);
                    inline.push("\n", InlineStyle::default(), None);
                }
            }
            ("b" | "strong", false) => self.bold += 1,
            ("b" | "strong", true) => self.bold = self.bold.saturating_sub(1),
            ("i" | "em" | "cite" | "var", false) => self.italic += 1,
            ("i" | "em" | "cite" | "var", true) => self.italic = self.italic.saturating_sub(1),
            ("s" | "del" | "strike", false) => self.strikethrough += 1,
            ("s" | "del" | "strike", true) => {
                self.strikethrough = self.strikethrough.saturating_sub(1)
            }
            ("code" | "kbd" | "tt" | "samp", false) => self.code_inline += 1,
            ("code" | "kbd" | "tt" | "samp", true) => {
                self.code_inline = self.code_inline.saturating_sub(1)
            }
            ("a", false) => self.links.push(
                attr(attrs, "href")
                    .map(str::to_string)
                    .filter(|h| !h.is_empty()),
            ),
            ("a", true) => {
                self.links.pop();
            }
            ("img", false) => {
                let src = attr(attrs, "src").unwrap_or_default().to_string();
                let alt = attr(attrs, "alt").unwrap_or_default().to_string();
                let width = pixel_attr(attrs, "width");
                let height = pixel_attr(attrs, "height");
                self.push_image(src, alt, width, height);
            }
            ("p" | "div" | "center", false) => {
                self.flush_inline();
                let align = if name == "center" {
                    Some(Align::Center)
                } else {
                    attr(attrs, "align").and_then(Align::from_attr)
                };
                self.open_aligned(name, align);
            }
            ("p" | "div" | "center", true) => {
                self.flush_inline();
                self.close_aligned(name);
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                self.flush_inline();
                self.open_aligned(name, attr(attrs, "align").and_then(Align::from_attr));
                let level = name.as_bytes()[1] - b'0';
                self.inline = Some((InlineTarget::Heading(level), RichText::default()));
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                self.flush_inline();
                self.close_aligned(name);
            }
            ("details", false) => self.push_frame(
                Container::Details {
                    open: attr(attrs, "open").is_some(),
                    summary: None,
                },
                true,
            ),
            ("details", true) => self.pop_html_frame(|c| matches!(c, Container::Details { .. })),
            ("summary", false) => {
                self.flush_inline();
                self.inline = Some((InlineTarget::Summary, RichText::default()));
            }
            ("summary", true) => {
                if matches!(self.inline, Some((InlineTarget::Summary, _))) {
                    self.flush_inline();
                }
            }
            ("blockquote", false) => self.push_frame(Container::Quote, true),
            ("blockquote", true) => self.pop_html_frame(|c| matches!(c, Container::Quote)),
            ("ul" | "ol", false) => {
                let start = (name == "ol").then(|| {
                    attr(attrs, "start")
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(1)
                });
                self.push_frame(
                    Container::List {
                        start,
                        items: Vec::new(),
                    },
                    true,
                );
            }
            ("ul" | "ol", true) => self.pop_html_frame(|c| matches!(c, Container::List { .. })),
            ("li", false) => {
                if self
                    .frames
                    .last()
                    .is_some_and(|f| f.html && matches!(f.container, Container::Item { .. }))
                {
                    self.pop_one();
                }
                self.push_frame(Container::Item { task: None }, true);
            }
            ("li", true) => self.pop_html_frame(|c| matches!(c, Container::Item { .. })),
            ("hr", _) => self.push_block(Block::Rule),
            ("pre", false) => {
                self.flush_inline();
                self.code = Some((None, String::new()));
                self.code_html = true;
            }
            ("table", false) => {
                self.flush_inline();
                self.tables.push(TableBuild {
                    align: Vec::new(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                    html_row_is_head: false,
                    html: true,
                });
            }
            ("table", true) => {
                if self.tables.last().is_some_and(|t| t.html) {
                    self.end_html_row();
                    self.end_table();
                }
            }
            ("tr", false) => {
                if self.tables.last().is_some_and(|t| t.html) {
                    self.end_html_row();
                    if let Some(table) = self.tables.last_mut() {
                        table.rows.push(Vec::new());
                        table.html_row_is_head = true;
                    }
                }
            }
            ("tr", true) => self.end_html_row(),
            ("td" | "th", false) => {
                if self.tables.last().is_some_and(|t| t.html) {
                    self.flush_inline();
                    if let Some(table) = self.tables.last_mut() {
                        if table.rows.is_empty() {
                            table.rows.push(Vec::new());
                            table.html_row_is_head = true;
                        }
                        if name == "td" {
                            table.html_row_is_head = false;
                        }
                        let column = table.rows.last().map_or(0, Vec::len);
                        if table.align.len() <= column {
                            table.align.resize(column + 1, None);
                        }
                        if let Some(align) = attr(attrs, "align").and_then(Align::from_attr) {
                            table.align[column].get_or_insert(align);
                        }
                    }
                    self.inline = Some((InlineTarget::Cell, RichText::default()));
                }
            }
            ("td" | "th", true) => {
                if matches!(self.inline, Some((InlineTarget::Cell, _))) {
                    self.flush_inline();
                }
            }
            // `<picture>`, `<source>`, `<span>`, `<sub>`, `<sup>`, `<ins>`,
            // `<g-emoji>`…: their text stays
            _ => {}
        }
    }

    /// `<p align>` and friends: an aligned frame when aligned.
    fn open_aligned(&mut self, tag: &str, align: Option<Align>) {
        let aligned = align.is_some_and(|a| a != Align::Left);
        if let Some(align) = align.filter(|_| aligned) {
            self.push_frame(Container::Aligned(align), true);
        }
        self.html_open.push(HtmlOpen {
            tag: tag.to_string(),
            aligned,
        });
    }

    fn close_aligned(&mut self, tag: &str) {
        let Some(ix) = self.html_open.iter().rposition(|o| o.tag == tag) else {
            return;
        };
        let open = self.html_open.remove(ix);
        self.html_open.truncate(ix);
        if open.aligned {
            self.pop_html_frame(|c| matches!(c, Container::Aligned(_)));
        }
    }

    /// An HTML table row ends: a first row of `<th>` cells is the head.
    fn end_html_row(&mut self) {
        if matches!(self.inline, Some((InlineTarget::Cell, _))) {
            self.flush_inline();
        }
        if let Some(table) = self.tables.last_mut()
            && table.html
            && table.html_row_is_head
            && table.head.is_empty()
            && table.rows.len() == 1
            && table.rows[0].iter().any(|c| !c.is_empty())
        {
            table.head = table.rows.pop().unwrap_or_default();
        }
        if let Some(table) = self.tables.last_mut() {
            table.html_row_is_head = false;
        }
    }

    fn end_table(&mut self) {
        self.flush_inline();
        let Some(table) = self.tables.pop() else {
            return;
        };
        let columns = table
            .rows
            .iter()
            .map(Vec::len)
            .chain([table.head.len(), table.align.len()])
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return;
        }
        let pad = |mut row: Vec<RichText>| {
            row.resize(columns, RichText::default());
            row
        };
        let mut align = table.align;
        align.resize(columns, None);
        let head = if table.head.is_empty() {
            Vec::new()
        } else {
            pad(table.head)
        };
        let rows = table
            .rows
            .into_iter()
            .filter(|r| !r.is_empty())
            .map(pad)
            .collect();
        self.push_block(Block::Table { align, head, rows });
    }

    fn end_code_block(&mut self) {
        let html = std::mem::take(&mut self.code_html);
        if let Some((language, mut code)) = self.code.take() {
            // `<pre>` drops the newline right after its start tag
            if html && code.starts_with('\n') {
                code.remove(0);
            }
            if code.ends_with('\n') {
                code.pop();
            }
            if self.math && language.as_deref() == Some("math") {
                self.push_block(Block::Math {
                    tex: code,
                    number: None,
                });
                return;
            }
            self.push_block(Block::CodeBlock { language, code });
        }
    }

    fn footnote_number(&mut self, label: &str) -> usize {
        match self.footnote_order.iter().position(|l| l == label) {
            Some(ix) => ix + 1,
            None => {
                self.footnote_order.push(label.to_string());
                self.footnote_order.len()
            }
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                if let Some(image) = &mut self.image {
                    image.alt.push_str(&code);
                    return;
                }
                let mut style = self.style();
                style.code = true;
                let link = self.link();
                self.inline().push(&code, style, link.as_deref());
            }
            Event::Html(html) => self.html(&html, true),
            Event::InlineHtml(html) => self.html(&html, false),
            Event::InlineMath(m) | Event::DisplayMath(m) if !self.math => self.text(&m),
            Event::InlineMath(m) => self.push_math(&m, false),
            Event::DisplayMath(m) => self.push_math(&m, true),
            Event::FootnoteReference(label) => {
                let number = self.footnote_number(&label);
                let link = format!("#user-content-fn-{label}");
                let style = self.style();
                self.inline()
                    .push(&format!("[{number}]"), style, Some(&link));
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, code)) = &mut self.code {
                    code.push('\n');
                } else if let Some(image) = &mut self.image {
                    image.alt.push(' ');
                } else {
                    self.inline().push("\n", InlineStyle::default(), None);
                }
            }
            Event::Rule => self.push_block(Block::Rule),
            Event::TaskListMarker(checked) => {
                if let Some(Frame {
                    container: Container::Item { task },
                    ..
                }) = self
                    .frames
                    .iter_mut()
                    .rev()
                    .find(|f| matches!(f.container, Container::Item { .. }) && !f.html)
                {
                    *task = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush_inline();
                self.inline = Some((InlineTarget::Paragraph, RichText::default()));
            }
            Tag::Heading { level, .. } => {
                self.flush_inline();
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.inline = Some((InlineTarget::Heading(level), RichText::default()));
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .filter(|l| !l.is_empty())
                        .map(str::to_string),
                    CodeBlockKind::Indented => None,
                };
                self.code = Some((language, String::new()));
                self.code_html = false;
            }
            Tag::BlockQuote(kind) => {
                let container = match kind {
                    None => Container::Quote,
                    Some(kind) => Container::Alert(match kind {
                        BlockQuoteKind::Note => AlertKind::Note,
                        BlockQuoteKind::Tip => AlertKind::Tip,
                        BlockQuoteKind::Important => AlertKind::Important,
                        BlockQuoteKind::Warning => AlertKind::Warning,
                        BlockQuoteKind::Caution => AlertKind::Caution,
                    }),
                };
                self.push_frame(container, false)
            }
            Tag::List(start) => self.push_frame(
                Container::List {
                    start,
                    items: Vec::new(),
                },
                false,
            ),
            Tag::Item => self.push_frame(Container::Item { task: None }, false),
            Tag::FootnoteDefinition(label) => {
                self.push_frame(Container::Footnote(label.to_string()), false)
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strikethrough += 1,
            Tag::Link { dest_url, .. } => self.links.push(Some(dest_url.to_string())),
            Tag::Image { dest_url, .. } => {
                let link = self.link();
                self.image = Some(PendingImage {
                    src: dest_url.to_string(),
                    alt: String::new(),
                    link,
                });
            }
            Tag::Table(align) => {
                self.flush_inline();
                self.tables.push(TableBuild {
                    align: align
                        .into_iter()
                        .map(|a| match a {
                            Alignment::None => None,
                            Alignment::Left => Some(Align::Left),
                            Alignment::Center => Some(Align::Center),
                            Alignment::Right => Some(Align::Right),
                        })
                        .collect(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                    html_row_is_head: false,
                    html: false,
                });
            }
            Tag::TableHead => {
                if let Some(table) = self.tables.last_mut() {
                    table.in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(table) = self.tables.last_mut() {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                self.flush_inline();
                self.inline = Some((InlineTarget::Cell, RichText::default()));
            }
            // definition lists, metadata, sub/superscript: their text
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.flush_inline(),
            TagEnd::HtmlBlock => {
                // a `<summary>` or table cell may continue in the next block
                if matches!(self.inline, Some((InlineTarget::Paragraph, _))) {
                    self.flush_inline();
                }
            }
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::FootnoteDefinition => {
                self.pop_frame()
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strikethrough = self.strikethrough.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                if let Some(image) = self.image.take() {
                    let saved = std::mem::replace(&mut self.links, vec![image.link]);
                    self.push_image(image.src, image.alt, None, None);
                    self.links = saved;
                }
            }
            TagEnd::TableCell => {
                if matches!(self.inline, Some((InlineTarget::Cell, _))) {
                    self.flush_inline();
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.tables.last_mut() {
                    table.in_head = false;
                }
            }
            TagEnd::Table => self.end_table(),
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        if self.code.is_some() {
            self.end_code_block();
        }
        while !self.tables.is_empty() {
            self.end_table();
        }
        self.flush_inline();
        while self.frames.len() > 1 {
            self.pop_one();
        }
        let mut blocks = self.frames.pop().map(|f| f.blocks).unwrap_or_default();
        if !self.footnotes.is_empty() {
            // GitHub lists the referenced definitions, in reference order
            let mut notes = Vec::new();
            for label in &self.footnote_order {
                if let Some(ix) = self.footnotes.iter().position(|(l, _)| l == label) {
                    notes.push(self.footnotes.remove(ix).1);
                }
            }
            if !notes.is_empty() {
                blocks.push(Block::Footnotes(notes));
            }
        }
        blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_message_code_urls_and_shas() {
        let base = "https://github.com/o/r";
        let t = commit_message_rich_text(
            "Fix `foo()` per a5c3785 and https://x.io/a. Not `open\nor` 1234567 or defaced",
            None,
            TokenOptions::default(),
            true,
            Some(base),
        );
        assert_eq!(
            t.text,
            "Fix foo() per a5c3785 and https://x.io/a. Not `open\nor` 1234567 or defaced"
        );
        let spans: Vec<(&str, bool, Option<&str>)> = t
            .spans
            .iter()
            .map(|s| (&t.text[s.range.clone()], s.style.code, s.link.as_deref()))
            .collect();
        assert_eq!(
            spans,
            [
                ("foo()", true, None),
                (
                    "a5c3785",
                    false,
                    Some("https://github.com/o/r/commit/a5c3785")
                ),
                ("https://x.io/a", false, Some("https://x.io/a")),
            ]
        );
        // no base: SHAs stay plain
        let t = commit_message_rich_text("see a5c3785", None, TokenOptions::default(), true, None);
        assert!(t.spans.is_empty());
    }

    #[test]
    fn commit_message_ghd_tokens() {
        let repo = TokenRepository {
            html_url: "https://github.com/o/r".into(),
            web_base: "https://github.com".into(),
        };
        let spans = |t: &RichText| -> Vec<(String, Option<String>)> {
            t.spans
                .iter()
                .map(|s| (t.text[s.range.clone()].to_string(), s.link.clone()))
                .collect()
        };
        let msg = ":tada: `x` a5c3785 @me (#452) https://x.io/a.";
        // GHD: emoji, issue, mention, whole-word URL; backticks and SHAs plain
        let t = commit_message_rich_text(msg, Some(&repo), TokenOptions::default(), false, None);
        assert_eq!(t.text, "🎉 `x` a5c3785 @me (#452) https://x.io/a.");
        assert_eq!(
            spans(&t),
            [
                ("@me".into(), Some("https://github.com/me".into())),
                (
                    "#452".into(),
                    Some("https://github.com/o/r/issues/452".into())
                ),
                ("https://x.io/a.".into(), Some("https://x.io/a.".into())),
            ]
        );
        // `804` on top: code span, SHA, URL without the full stop
        let t = commit_message_rich_text(
            msg,
            Some(&repo),
            TokenOptions::default(),
            true,
            Some("https://github.com/o/r"),
        );
        assert_eq!(t.text, "🎉 x a5c3785 @me (#452) https://x.io/a.");
        assert_eq!(spans(&t)[0], ("x".into(), None));
        assert_eq!(
            spans(&t)[1..],
            [
                (
                    "a5c3785".into(),
                    Some("https://github.com/o/r/commit/a5c3785".into())
                ),
                ("@me".into(), Some("https://github.com/me".into())),
                (
                    "#452".into(),
                    Some("https://github.com/o/r/issues/452".into())
                ),
                ("https://x.io/a".into(), Some("https://x.io/a".into())),
            ]
        );
    }

    fn para(blocks: &[Block], ix: usize) -> &RichText {
        match &blocks[ix] {
            Block::Paragraph(t) => t,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn headings_and_paragraphs() {
        let blocks = parse("# Title\n\nSome text\nnext line\n\n### Small");
        assert_eq!(blocks.len(), 3);
        assert!(matches!(&blocks[0], Block::Heading { level: 1, text } if text.text == "Title"));
        // `breaks: true`: a single newline is a line break
        assert_eq!(para(&blocks, 1).text, "Some text\nnext line");
        assert!(matches!(&blocks[2], Block::Heading { level: 3, .. }));
    }

    #[test]
    fn inline_styles_are_sorted_non_overlapping_spans() {
        let blocks = parse("a **bold _both_** `code` ~~gone~~ z");
        let t = para(&blocks, 0);
        assert_eq!(t.text, "a bold both code gone z");
        let styles: Vec<(&str, InlineStyle)> = t
            .spans
            .iter()
            .map(|s| (&t.text[s.range.clone()], s.style))
            .collect();
        assert_eq!(styles[0].0, "bold ");
        assert!(styles[0].1.bold && !styles[0].1.italic);
        assert_eq!(styles[1].0, "both");
        assert!(styles[1].1.bold && styles[1].1.italic);
        assert_eq!(styles[2].0, "code");
        assert!(styles[2].1.code);
        assert_eq!(styles[3].0, "gone");
        assert!(styles[3].1.strikethrough);
        for w in t.spans.windows(2) {
            assert!(w[0].range.end <= w[1].range.start);
        }
    }

    #[test]
    fn links_and_bare_urls() {
        let blocks = parse("See [the docs](https://example.com/docs) or https://github.com/x.");
        let t = para(&blocks, 0);
        let links: Vec<(&str, &str)> = t.links().map(|(r, l)| (&t.text[r.clone()], l)).collect();
        assert_eq!(
            links,
            vec![
                ("the docs", "https://example.com/docs"),
                ("https://github.com/x", "https://github.com/x"),
            ]
        );
        assert!(t.text.ends_with("https://github.com/x."));
    }

    #[test]
    fn fenced_code_keeps_language_and_text() {
        let blocks = parse("```rust\nfn main() {}\n  x\n```\n");
        assert_eq!(
            blocks,
            vec![Block::CodeBlock {
                language: Some("rust".into()),
                code: "fn main() {}\n  x".into()
            }]
        );
    }

    #[test]
    fn tight_and_loose_lists_nest() {
        let blocks = parse("- one\n- two\n  1. inner\n  2. more\n\n3. three\n");
        let Block::List { start, items } = &blocks[0] else {
            panic!("expected a list: {blocks:?}")
        };
        assert_eq!(*start, None);
        assert_eq!(items.len(), 2);
        assert_eq!(para(&items[0].blocks, 0).text, "one");
        let Block::List {
            start,
            items: inner,
        } = &items[1].blocks[1]
        else {
            panic!("expected a nested list: {:?}", items[1])
        };
        assert_eq!(*start, Some(1));
        assert_eq!(inner.len(), 2);
        assert!(matches!(&blocks[1], Block::List { start: Some(3), .. }));
    }

    #[test]
    fn block_quotes_hold_blocks() {
        let blocks = parse("> quoted **text**\n>\n> - item\n\nafter");
        let Block::BlockQuote(inner) = &blocks[0] else {
            panic!("expected a quote: {blocks:?}")
        };
        assert_eq!(para(inner, 0).text, "quoted text");
        assert!(matches!(inner[1], Block::List { .. }));
        assert_eq!(para(&blocks, 1).text, "after");
    }

    #[test]
    fn inline_html_styles_and_breaks() {
        let blocks = parse("line<br>next <b>bold</b> <kbd>Ctrl</kbd> <!-- hidden --> end");
        let t = para(&blocks, 0);
        assert_eq!(t.text, "line\nnext bold Ctrl  end");
        let styled: Vec<(&str, InlineStyle)> = t
            .spans
            .iter()
            .map(|s| (&t.text[s.range.clone()], s.style))
            .collect();
        assert!(styled[0].0 == "bold" && styled[0].1.bold);
        assert!(styled[1].0 == "Ctrl" && styled[1].1.code);
    }

    #[test]
    fn images_markdown_and_html() {
        let blocks = parse(
            "![a screenshot](https://x/y.png) and [![badge](b.svg)](https://ci)\n\n<img src=\"/z.png\" alt=\"Z\" width=\"300\" height=\"50%\">",
        );
        let t = para(&blocks, 0);
        assert_eq!(t.text, " and ");
        assert_eq!(t.images.len(), 2);
        assert_eq!(t.images[0].at, 0);
        assert_eq!(t.images[0].alt, "a screenshot");
        assert_eq!(t.images[0].src, "https://x/y.png");
        assert_eq!(t.images[1].at, 5);
        assert_eq!(t.images[1].link.as_deref(), Some("https://ci"));
        let Block::Paragraph(t) = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(t.images[0].width, Some(300));
        assert_eq!(t.images[0].height, None);
        assert_eq!(t.images[0].alt, "Z");
    }

    #[test]
    fn picture_and_centered_html_blocks() {
        let md = "<p align=\"center\">\n  <picture>\n    <source srcset=\"dark.png\">\n    <img src=\"logo.png\" width=\"120\">\n  </picture>\n  <br>\n  <b>Corvene</b> &amp; friends\n</p>\n\nafter";
        let blocks = parse(md);
        let Block::Aligned {
            align,
            blocks: inner,
        } = &blocks[0]
        else {
            panic!("{blocks:?}")
        };
        assert_eq!(*align, Align::Center);
        let t = para(inner, 0);
        assert_eq!(t.images.len(), 1);
        assert_eq!(t.images[0].src, "logo.png");
        assert_eq!(t.text, "\nCorvene & friends");
        assert_eq!(para(&blocks, 1).text, "after");
    }

    #[test]
    fn details_hold_markdown_and_nest() {
        let md = "<details>\n<summary>Logs <b>here</b></summary>\n\nSome **text**\n\n<details open><summary>Inner</summary>\n\n- a\n\n</details>\n</details>\n\nafter";
        let blocks = parse(md);
        let Block::Details {
            open,
            summary,
            blocks: inner,
        } = &blocks[0]
        else {
            panic!("{blocks:?}")
        };
        assert!(!open);
        assert_eq!(summary.text, "Logs here");
        assert_eq!(para(inner, 0).text, "Some text");
        let Block::Details {
            open,
            summary,
            blocks: nested,
        } = &inner[1]
        else {
            panic!("{inner:?}")
        };
        assert!(open);
        assert_eq!(summary.text, "Inner");
        assert!(matches!(nested[0], Block::List { .. }));
        assert_eq!(para(&blocks, 1).text, "after");

        // all on one line, never closed
        let blocks = parse("<details><summary>S</summary>body text");
        let Block::Details {
            summary, blocks, ..
        } = &blocks[0]
        else {
            panic!("{blocks:?}")
        };
        assert_eq!(summary.text, "S");
        assert_eq!(para(blocks, 0).text, "body text");
    }

    #[test]
    fn tables_keep_cells_and_alignment() {
        let blocks = parse("| a | b | c |\n|---|:-:|--:|\n| 1 | **2** | 3 |\n| x |\n");
        let Block::Table { align, head, rows } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(*align, vec![None, Some(Align::Center), Some(Align::Right)]);
        assert_eq!(
            head.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1].text, "2");
        assert!(rows[0][1].spans[0].style.bold);
        assert_eq!(rows[1].len(), 3);

        let blocks = parse(
            "<table><tr><th>H</th><th>I</th></tr><tr><td align=\"right\">1</td><td>2</td></tr></table>",
        );
        let Block::Table { align, head, rows } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(head.len(), 2);
        assert_eq!(head[0].text, "H");
        assert_eq!(rows, &vec![vec![rows[0][0].clone(), rows[0][1].clone()]]);
        assert_eq!(rows[0][0].text, "1");
        assert_eq!(align[0], Some(Align::Right));
    }

    #[test]
    fn task_lists_alerts_and_footnotes() {
        let blocks = parse("- [x] done\n- [ ] open\n- plain");
        let Block::List { items, .. } = &blocks[0] else {
            panic!()
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(para(&items[0].blocks, 0).text, "done");
        assert_eq!(items[1].task, Some(false));
        assert_eq!(items[2].task, None);

        let blocks = parse("> [!WARNING]\n> Careful");
        let Block::Alert {
            kind,
            blocks: inner,
        } = &blocks[0]
        else {
            panic!("{blocks:?}")
        };
        assert_eq!(*kind, AlertKind::Warning);
        assert_eq!(para(inner, 0).text, "Careful");

        let blocks = parse("A[^b] and B[^a]\n\n[^a]: first\n[^b]: second\n");
        assert_eq!(para(&blocks, 0).text, "A[1] and B[2]");
        let Block::Footnotes(notes) = blocks.last().unwrap() else {
            panic!("{blocks:?}")
        };
        assert_eq!(para(&notes[0], 0).text, "second");
        assert_eq!(para(&notes[1], 0).text, "first");
    }

    #[test]
    fn rich_text_slices_keep_spans() {
        let blocks = parse("a **bold** z");
        let t = para(&blocks, 0);
        let s = t.slice(3..t.text.len());
        assert_eq!(s.text, "old z");
        assert_eq!(s.spans[0].range, 0..3);
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            decode_entities("a &amp; b &#x41;&#66; &nope; &"),
            "a & b AB &nope; &"
        );
    }

    #[test]
    fn links_resolve_like_ghds_interceptor() {
        let base = Some("https://github.com/desktop/desktop/pull/1");
        assert_eq!(
            resolve_link("https://x.dev/a", base).as_deref(),
            Some("https://x.dev/a")
        );
        assert_eq!(
            resolve_link("/desktop/desktop/issues/2", base).as_deref(),
            Some("https://github.com/desktop/desktop/issues/2")
        );
        assert_eq!(
            resolve_link("files", base).as_deref(),
            Some("https://github.com/desktop/desktop/pull/files")
        );
        assert_eq!(
            resolve_link("#top", base).as_deref(),
            Some("https://github.com/desktop/desktop/pull/1#top")
        );
        assert_eq!(resolve_link("mailto:a@b.c", base), None);
        assert_eq!(resolve_link("/relative", None), None);
    }

    #[test]
    fn plain_text_joins_blocks() {
        let blocks = parse("# T\n\n- a\n- b\n\n```\ncode\n```");
        assert_eq!(plain_text(&blocks), "T\na\nb\ncode");
    }
}

#[cfg(test)]
mod math_tests {
    use super::*;

    fn math(md: &str) -> Vec<Block> {
        parse_with(md, ParseOptions { math: true })
    }

    #[test]
    fn inline_and_display_math() {
        let blocks = math("Euler: $e^{i\\pi} + 1 = 0$ holds.\n\n$$\\int_0^1 x\\,dx$$\n");
        let Block::Paragraph(text) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(text.text, "Euler:  holds.");
        assert_eq!(text.maths.len(), 1);
        assert_eq!(text.maths[0].tex, "e^{i\\pi} + 1 = 0");
        assert_eq!(text.maths[0].at, 7);
        assert!(!text.maths[0].display);
        assert_eq!(
            blocks[1],
            Block::Math {
                tex: "\\int_0^1 x\\,dx".into(),
                number: None
            }
        );
    }

    #[test]
    fn math_off_keeps_text() {
        let blocks = parse("cost $5 and $x^2$");
        let Block::Paragraph(text) = &blocks[0] else {
            panic!()
        };
        assert!(text.maths.is_empty());
        assert!(text.text.contains("x^2"));
    }

    #[test]
    fn fences_and_environments() {
        let blocks = math(
            "```math\na^2+b^2=c^2\n```\n\n\\begin{align}\na &= b\\\\\nc &= d\n\\end{align}\n\ntext\n",
        );
        assert!(matches!(&blocks[0], Block::Math { tex, .. } if tex == "a^2+b^2=c^2"));
        assert!(
            matches!(&blocks[1], Block::Math { tex, .. } if tex.starts_with("\\begin{align}") && tex.ends_with("\\end{align}")),
            "{:?}",
            blocks[1]
        );
        assert!(matches!(&blocks[2], Block::Paragraph(t) if t.text == "text"));
        // not inside code
        let code = math("```\n\\begin{align}\nx\n\\end{align}\n```\n");
        assert!(matches!(&code[0], Block::CodeBlock { .. }));
    }
}
