//! GHD `Terminal` (`ui/terminal.tsx`): the read-only xterm.js 5.5 terminal
//! the HookFailed and Committing changes dialogs show git's output in.
//! xterm's defaults as GHD sets them: `convertEol` (a line feed also returns
//! the carriage), 12 px of GHD's monospace stack, white on black, 1000 lines
//! of scrollback, the cursor hidden; `.xterm { padding: var(--spacing) }`.
//!
//! [`TerminalBuffer`] interprets what xterm does with git and hook output:
//! printable text with wrapping at the last column, carriage returns,
//! backspaces, tabs, SGR colours (16, 256 and true colour; bold draws the
//! bright colours, `drawBoldTextInBrightColors`), erasing in the line and
//! the display and cursor movement. Other escape sequences are dropped.
//!
//! Deviations: characters take one cell each (xterm gives East Asian wide
//! characters and emoji two), and the text cannot be selected or copied.

use std::collections::VecDeque;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, mono_font};

/// xterm's `scrollback` default.
const SCROLLBACK: usize = 1000;
/// SF Mono's advance at 12 px, xterm's cell width.
const CELL_WIDTH: f32 = 7.225;
/// xterm's cell height for 12 px SF Mono (`lineHeight: 1`).
const CELL_HEIGHT: f32 = 14.;
const FONT_SIZE: f32 = 12.;
/// The viewport's classic scrollbar (`.xterm-viewport { overflow-y: scroll }`).
const SCROLLBAR_WIDTH: f32 = 15.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pen {
    fg: Color,
    bg: Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    ch: char,
    pen: Pen,
}

const BLANK: Cell = Cell {
    ch: ' ',
    pen: Pen {
        fg: Color::Default,
        bg: Color::Default,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        inverse: false,
    },
};

#[derive(Clone, Debug, Default)]
enum Parse {
    #[default]
    Ground,
    Escape,
    /// `ESC (` and friends: one more byte names the charset.
    Charset,
    Csi {
        params: String,
        private: bool,
    },
    /// `ESC ]` … `BEL` / `ESC \`.
    Osc {
        escape: bool,
    },
}

/// The terminal's lines and cursor (xterm's `Buffer`).
#[derive(Clone, Debug)]
pub struct TerminalBuffer {
    cols: usize,
    rows: usize,
    /// Scrollback, then the `rows` lines of the screen.
    lines: VecDeque<Vec<Cell>>,
    x: usize,
    /// The cursor's row on the screen.
    y: usize,
    pen: Pen,
    parse: Parse,
    /// The start of a UTF-8 sequence split across writes.
    pending: Vec<u8>,
}

impl TerminalBuffer {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            lines: (0..rows.max(1)).map(|_| Vec::new()).collect(),
            x: 0,
            y: 0,
            pen: Pen::default(),
            parse: Parse::default(),
            pending: Vec::new(),
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Lines in the buffer (scrollback and screen).
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Vec::is_empty)
    }

    /// The text of line `ix`, trailing blanks dropped (for tests and
    /// accessibility).
    pub fn line_text(&self, ix: usize) -> String {
        self.lines
            .get(ix)
            .map(|line| line.iter().map(|c| c.ch).collect::<String>())
            .unwrap_or_default()
            .trim_end()
            .to_string()
    }

    /// Everything on screen and in the scrollback.
    pub fn text(&self) -> String {
        let mut lines: Vec<String> = (0..self.lines.len()).map(|i| self.line_text(i)).collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Every line as text with its styled runs (`1331-notebook-diff`:
    /// notebook streams and tracebacks), trailing blank lines dropped.
    pub fn styled_lines(&self) -> Vec<(String, Vec<(std::ops::Range<usize>, HighlightStyle)>)> {
        let mut out: Vec<_> = self.lines.iter().map(|l| line_runs(l)).collect();
        for (text, runs) in &mut out {
            let trimmed = text.trim_end().len();
            text.truncate(trimmed);
            runs.retain(|(r, _)| r.start < trimmed);
            for (r, _) in runs.iter_mut() {
                r.end = r.end.min(trimmed);
            }
        }
        while out.last().is_some_and(|(t, _)| t.is_empty()) {
            out.pop();
        }
        out
    }

    fn ybase(&self) -> usize {
        self.lines.len() - self.rows
    }

    /// xterm `write`.
    pub fn write(&mut self, bytes: &[u8]) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut rest = &data[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    self.write_str(text);
                    break;
                }
                Err(err) => {
                    let (valid, after) = rest.split_at(err.valid_up_to());
                    // SAFETY-free: `valid_up_to` bytes are valid UTF-8
                    self.write_str(std::str::from_utf8(valid).unwrap_or_default());
                    match err.error_len() {
                        // the sequence goes on in the next write
                        None => {
                            self.pending = after.to_vec();
                            break;
                        }
                        Some(len) => {
                            self.write_str("\u{fffd}");
                            rest = &after[len..];
                        }
                    }
                }
            }
        }
    }

    fn write_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.feed(ch);
        }
    }

    fn feed(&mut self, ch: char) {
        match std::mem::take(&mut self.parse) {
            Parse::Ground => self.ground(ch),
            Parse::Escape => match ch {
                '[' => {
                    self.parse = Parse::Csi {
                        params: String::new(),
                        private: false,
                    }
                }
                ']' => self.parse = Parse::Osc { escape: false },
                '(' | ')' | '*' | '+' => self.parse = Parse::Charset,
                // `ESC E`: next line
                'E' => {
                    self.x = 0;
                    self.line_feed();
                }
                // `ESC D`: index
                'D' => self.line_feed(),
                // `ESC M`: reverse index
                'M' => self.y = self.y.saturating_sub(1),
                _ => {}
            },
            Parse::Charset => {}
            Parse::Csi {
                mut params,
                mut private,
            } => match ch {
                '0'..='9' | ';' | ':' => {
                    params.push(ch);
                    self.parse = Parse::Csi { params, private };
                }
                '?' | '>' | '=' | '<' | ' ' | '!' | '"' | '$' | '\'' => {
                    private = true;
                    self.parse = Parse::Csi { params, private };
                }
                '\u{40}'..='\u{7e}' => {
                    if !private {
                        self.csi(ch, &params);
                    }
                }
                // C0 controls run inside a sequence
                '\u{0}'..='\u{1f}' => {
                    self.parse = Parse::Csi { params, private };
                    self.ground(ch);
                }
                _ => {}
            },
            Parse::Osc { escape } => match ch {
                '\u{7}' => {}
                '\\' if escape => {}
                '\u{1b}' => self.parse = Parse::Osc { escape: true },
                _ => self.parse = Parse::Osc { escape: false },
            },
        }
    }

    fn ground(&mut self, ch: char) {
        match ch {
            '\u{1b}' => self.parse = Parse::Escape,
            // `convertEol`
            '\n' | '\u{b}' | '\u{c}' => {
                self.x = 0;
                self.line_feed();
            }
            '\r' => self.x = 0,
            '\u{8}' => self.x = self.x.min(self.cols - 1).saturating_sub(1),
            '\t' => self.x = ((self.x / 8 + 1) * 8).min(self.cols - 1),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {}
            ch => self.print(ch),
        }
    }

    fn line_feed(&mut self) {
        if self.y + 1 < self.rows {
            self.y += 1;
            return;
        }
        self.lines.push_back(Vec::new());
        if self.lines.len() > self.rows + SCROLLBACK {
            self.lines.pop_front();
        }
    }

    fn print(&mut self, ch: char) {
        // xterm wraps once a character goes past the last column
        if self.x >= self.cols {
            self.x = 0;
            self.line_feed();
        }
        let row = self.ybase() + self.y;
        let line = &mut self.lines[row];
        if line.len() <= self.x {
            line.resize(self.x + 1, BLANK);
        }
        line[self.x] = Cell { ch, pen: self.pen };
        self.x += 1;
    }

    fn params(params: &str) -> Vec<u16> {
        params
            .split([';', ':'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    }

    fn csi(&mut self, final_byte: char, params: &str) {
        let p = Self::params(params);
        let n = |i: usize| p.get(i).copied().filter(|v| *v > 0).unwrap_or(1) as usize;
        let mode = p.first().copied().unwrap_or(0);
        match final_byte {
            'm' => self.sgr(&p),
            'A' => self.y = self.y.saturating_sub(n(0)),
            'B' | 'e' => self.y = (self.y + n(0)).min(self.rows - 1),
            'C' | 'a' => self.x = (self.x + n(0)).min(self.cols - 1),
            'D' => self.x = self.x.min(self.cols - 1).saturating_sub(n(0)),
            'E' => {
                self.y = (self.y + n(0)).min(self.rows - 1);
                self.x = 0;
            }
            'F' => {
                self.y = self.y.saturating_sub(n(0));
                self.x = 0;
            }
            'G' | '`' => self.x = (n(0) - 1).min(self.cols - 1),
            'd' => self.y = (n(0) - 1).min(self.rows - 1),
            'H' | 'f' => {
                self.y = (n(0) - 1).min(self.rows - 1);
                self.x = (n(1) - 1).min(self.cols - 1);
            }
            'K' => {
                let row = self.ybase() + self.y;
                let x = self.x.min(self.cols);
                let line = &mut self.lines[row];
                match mode {
                    0 => line.truncate(x),
                    1 => {
                        for cell in line.iter_mut().take(x + 1) {
                            *cell = BLANK;
                        }
                    }
                    _ => line.clear(),
                }
            }
            'J' => {
                let base = self.ybase();
                let row = base + self.y;
                match mode {
                    0 => {
                        self.lines[row].truncate(self.x.min(self.cols));
                        for line in self.lines.iter_mut().skip(row + 1) {
                            line.clear();
                        }
                    }
                    1 => {
                        for line in self.lines.iter_mut().skip(base).take(self.y) {
                            line.clear();
                        }
                        for cell in self.lines[row].iter_mut().take(self.x + 1) {
                            *cell = BLANK;
                        }
                    }
                    _ => {
                        for line in self.lines.iter_mut().skip(base) {
                            line.clear();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn sgr(&mut self, p: &[u16]) {
        if p.is_empty() {
            self.pen = Pen::default();
            return;
        }
        let mut i = 0;
        while i < p.len() {
            match p[i] {
                0 => self.pen = Pen::default(),
                1 => self.pen.bold = true,
                2 => self.pen.dim = true,
                3 => self.pen.italic = true,
                4 => self.pen.underline = true,
                7 => self.pen.inverse = true,
                22 => {
                    self.pen.bold = false;
                    self.pen.dim = false;
                }
                23 => self.pen.italic = false,
                24 => self.pen.underline = false,
                27 => self.pen.inverse = false,
                c @ 30..=37 => self.pen.fg = Color::Indexed((c - 30) as u8),
                39 => self.pen.fg = Color::Default,
                c @ 40..=47 => self.pen.bg = Color::Indexed((c - 40) as u8),
                49 => self.pen.bg = Color::Default,
                c @ 90..=97 => self.pen.fg = Color::Indexed((c - 90 + 8) as u8),
                c @ 100..=107 => self.pen.bg = Color::Indexed((c - 100 + 8) as u8),
                c @ (38 | 48) => {
                    let color = match p.get(i + 1) {
                        Some(5) => {
                            let color = p.get(i + 2).map(|v| Color::Indexed(*v as u8));
                            i += 2;
                            color
                        }
                        Some(2) => {
                            let at = |k: usize| p.get(i + k).copied().unwrap_or(0) as u8;
                            let color = Some(Color::Rgb(at(2), at(3), at(4)));
                            i += 4;
                            color
                        }
                        _ => None,
                    };
                    if let Some(color) = color {
                        if c == 38 {
                            self.pen.fg = color;
                        } else {
                            self.pen.bg = color;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// xterm.js 5's default 16 colours.
const ANSI: [u32; 16] = [
    0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, 0x555753,
    0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
];
const FOREGROUND: u32 = 0xffffff;
const BACKGROUND: u32 = 0x000000;

fn indexed(ix: u8) -> u32 {
    match ix {
        0..=15 => ANSI[ix as usize],
        16..=231 => {
            const STEPS: [u32; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
            let v = (ix - 16) as usize;
            (STEPS[v / 36] << 16) | (STEPS[(v / 6) % 6] << 8) | STEPS[v % 6]
        }
        _ => {
            let g = 8 + 10 * (ix as u32 - 232);
            (g << 16) | (g << 8) | g
        }
    }
}

fn resolve(color: Color, bold: bool, default: u32) -> u32 {
    match color {
        Color::Default => default,
        // `drawBoldTextInBrightColors`
        Color::Indexed(ix) if bold && ix < 8 => indexed(ix + 8),
        Color::Indexed(ix) => indexed(ix),
        Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
    }
}

fn style(pen: Pen) -> HighlightStyle {
    let mut fg = resolve(pen.fg, pen.bold, FOREGROUND);
    let mut bg = match pen.bg {
        Color::Default => None,
        other => Some(resolve(other, false, BACKGROUND)),
    };
    if pen.inverse {
        let old_fg = fg;
        fg = bg.unwrap_or(BACKGROUND);
        bg = Some(old_fg);
    }
    let mut color: Hsla = rgb(fg).into();
    if pen.dim {
        color = color.opacity(0.5);
    }
    HighlightStyle {
        color: Some(color),
        background_color: bg.map(|bg| rgb(bg).into()),
        font_weight: pen.bold.then_some(FontWeight::BOLD),
        font_style: pen.italic.then_some(FontStyle::Italic),
        underline: pen.underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: false,
        }),
        ..Default::default()
    }
}

/// One line as text and its styled runs (default runs left out).
fn line_runs(line: &[Cell]) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut start = 0;
    let mut current: Option<Pen> = None;
    for cell in line {
        if current != Some(cell.pen) {
            if let Some(pen) = current
                && pen != Pen::default()
            {
                runs.push((start..text.len(), style(pen)));
            }
            start = text.len();
            current = Some(cell.pen);
        }
        text.push(cell.ch);
    }
    if let Some(pen) = current
        && pen != Pen::default()
    {
        runs.push((start..text.len(), style(pen)));
    }
    (text, runs)
}

/// The terminal view: a [`TerminalBuffer`] and where its viewport is.
pub struct Terminal {
    buffer: TerminalBuffer,
    /// The first line shown; `None` follows the output (xterm keeps the
    /// viewport at the bottom while it is there).
    top: Option<usize>,
    /// Wheel movement short of a whole line.
    wheel_rest: f32,
    /// Extra width on the right, so the scrollbar clears the last column
    /// (`1114-hook-results`; GHD's dialogs have none).
    right_gutter: f32,
}

impl Terminal {
    /// GHD `<Terminal rows cols terminalOutput>`.
    pub fn new(cols: usize, rows: usize, output: &[u8]) -> Self {
        let mut buffer = TerminalBuffer::new(cols, rows);
        buffer.write(output);
        Self {
            buffer,
            top: None,
            wheel_rest: 0.,
            right_gutter: 0.,
        }
    }

    /// `1114-hook-results`: as many columns as fit `width` (unzoomed px),
    /// at least 20.
    pub fn cols_for_width(width: f32) -> usize {
        (((width - SPACING_PX - SCROLLBAR_WIDTH) / CELL_WIDTH).floor() as usize).max(20)
    }

    /// `output` in `cols` columns, as high as it needs up to `max_rows`
    /// (with the cursor's line after a closing line feed).
    pub fn fitted(cols: usize, max_rows: usize, output: &[u8]) -> Self {
        let mut probe = TerminalBuffer::new(cols, max_rows.max(1));
        probe.write(output);
        let used = probe.text().lines().count() + usize::from(output.ends_with(b"\n"));
        Self {
            right_gutter: SCROLLBAR_WIDTH - SPACING_PX,
            ..Self::new(cols, used.clamp(1, max_rows.max(1)), output)
        }
    }

    /// GHD `Terminal.write`.
    pub fn write(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if bytes.is_empty() {
            return;
        }
        self.buffer.write(bytes);
        cx.notify();
    }

    pub fn buffer(&self) -> &TerminalBuffer {
        &self.buffer
    }

    fn max_top(&self) -> usize {
        self.buffer.ybase()
    }

    fn top(&self) -> usize {
        self.top.unwrap_or(self.max_top()).min(self.max_top())
    }

    fn scroll_lines(&mut self, lines: isize, cx: &mut Context<Self>) {
        let max = self.max_top() as isize;
        let top = (self.top() as isize + lines).clamp(0, max) as usize;
        self.top = (top as isize != max).then_some(top);
        cx.notify();
    }
}

impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.ghd().is_dark();
        let (rows, cols) = (self.buffer.rows, self.buffer.cols);
        let top = self.top();
        let lines: Vec<AnyElement> = (top..top + rows)
            .map(|ix| {
                let (text, runs) = self
                    .buffer
                    .lines
                    .get(ix)
                    .map(|line| line_runs(line))
                    .unwrap_or_default();
                div()
                    .h(zpx(CELL_HEIGHT))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(StyledText::new(SharedString::from(text)).with_highlights(runs))
                    .into_any_element()
            })
            .collect();
        let overflow = self.max_top() > 0;
        // Chromium's classic scrollbar track (GHD's dark and light themes
        // both set `color-scheme`)
        let (track, track_left, track_right) = if dark {
            (rgb(0x2c2c2c), rgb(0x3d3d3d), rgb(0x515151))
        } else {
            (rgb(0xf1f1f1), rgb(0xe7e7e7), rgb(0xe7e7e7))
        };
        let thumb = overflow.then(|| {
            let total = self.buffer.len() as f32;
            let height = CELL_HEIGHT * rows as f32 + 2. * SPACING_PX;
            let thumb_h = (height * rows as f32 / total).max(18.);
            let thumb_y = (height - thumb_h) * top as f32 / self.max_top().max(1) as f32;
            div()
                .absolute()
                .top(zpx(thumb_y))
                .left(zpx(3.))
                .w(zpx(SCROLLBAR_WIDTH - 6.))
                .h(zpx(thumb_h))
                .rounded(zpx(4.))
                .bg(if dark { rgb(0x6b6b6b) } else { rgb(0xc1c1c1) })
        });
        div()
            .id("terminal")
            .relative()
            .flex_none()
            .p(zpx(SPACING_PX))
            .w(zpx(CELL_WIDTH * cols as f32
                + 2. * SPACING_PX
                + self.right_gutter))
            .bg(rgb(BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .font_family(mono_font())
            .text_size(zpx(FONT_SIZE))
            .line_height(zpx(CELL_HEIGHT))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                let dy = match e.delta {
                    ScrollDelta::Lines(d) => -d.y * 3.,
                    ScrollDelta::Pixels(d) => -f32::from(d.y) / f32::from(zpx(CELL_HEIGHT)),
                };
                this.wheel_rest += dy;
                let whole = this.wheel_rest.trunc();
                this.wheel_rest -= whole;
                if whole != 0. {
                    this.scroll_lines(whole as isize, cx);
                }
            }))
            .child(
                div()
                    .h(zpx(CELL_HEIGHT * rows as f32))
                    .overflow_hidden()
                    .children(lines),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(zpx(SCROLLBAR_WIDTH))
                    .flex()
                    .flex_row()
                    .child(div().w(zpx(1.)).h_full().bg(track_left))
                    .child(div().flex_1().h_full().bg(track))
                    .child(div().w(zpx(1.)).h_full().bg(track_right))
                    .children(thumb),
            )
    }
}

/// `--spacing`.
const SPACING_PX: f32 = 10.;

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(output: &[u8]) -> String {
        let mut buffer = TerminalBuffer::new(80, 20);
        buffer.write(output);
        buffer.text()
    }

    #[::core::prelude::v1::test]
    fn line_feeds_return_the_carriage() {
        assert_eq!(screen(b"one\ntwo\n"), "one\ntwo");
    }

    #[::core::prelude::v1::test]
    fn carriage_returns_overwrite_the_line() {
        assert_eq!(screen(b"foo bar\rbaz"), "baz bar");
        assert_eq!(screen(b"Counting 10%\rCounting 100%\n"), "Counting 100%");
    }

    #[::core::prelude::v1::test]
    fn long_lines_wrap_at_the_last_column() {
        let mut buffer = TerminalBuffer::new(4, 3);
        buffer.write(b"abcdefg");
        assert_eq!(buffer.text(), "abcd\nefg");
    }

    #[::core::prelude::v1::test]
    fn colours_are_runs_and_escapes_are_not_text() {
        let mut buffer = TerminalBuffer::new(80, 5);
        buffer.write(b"\x1b[?25l\x1b[31merror\x1b[0m: \x1b]0;title\x07bad\x1b[K");
        assert_eq!(buffer.text(), "error: bad");
        let (_, runs) = line_runs(&buffer.lines[0]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, 0..5);
    }

    #[::core::prelude::v1::test]
    fn split_utf8_sequences_join_up() {
        let mut buffer = TerminalBuffer::new(80, 5);
        let bytes = "héllo ✓".as_bytes();
        buffer.write(&bytes[..2]);
        buffer.write(&bytes[2..]);
        assert_eq!(buffer.text(), "héllo ✓");
    }

    #[::core::prelude::v1::test]
    fn the_scrollback_keeps_a_thousand_lines() {
        let mut buffer = TerminalBuffer::new(80, 20);
        for i in 0..2000 {
            buffer.write(format!("{i}\n").as_bytes());
        }
        assert_eq!(buffer.len(), 20 + SCROLLBACK);
        assert_eq!(buffer.line_text(buffer.len() - 2), "1999");
    }

    #[::core::prelude::v1::test]
    fn the_erase_sequences_clear() {
        assert_eq!(screen(b"keep\nwipe this\x1b[2K\rnew"), "keep\nnew");
        assert_eq!(screen(b"a\nb\x1b[2J"), "");
    }
}

/// `1331-notebook-diff`: text with ANSI escapes (a notebook stream or
/// traceback) as styled lines, carriage returns overwriting their line as
/// in a terminal (progress bars collapse to their last state).
pub fn ansi_lines(
    text: &str,
    cols: usize,
) -> Vec<(String, Vec<(std::ops::Range<usize>, HighlightStyle)>)> {
    let mut buffer = TerminalBuffer::new(cols, 1);
    // a notebook's newlines are line feeds without carriage returns
    let bytes = text.replace("\r\n", "\n").replace('\n', "\r\n");
    buffer.write(bytes.as_bytes());
    buffer.styled_lines()
}
