//! The line writer every printer shares: indentation, collapsed spaces and
//! no blank lines.

/// Deepest indent written; deeper nesting stays at this column so output
/// size stays linear in the input.
const MAX_INDENT: usize = 40;
const SPACES: &str =
    "                                                                                ";

pub(crate) struct Out {
    pub(crate) buf: String,
    pub(crate) indent: usize,
    line_start: bool,
    pending_space: bool,
}

impl Out {
    pub(crate) fn with_capacity(capacity: usize) -> Out {
        Out {
            buf: String::with_capacity(capacity),
            indent: 0,
            line_start: true,
            pending_space: false,
        }
    }

    /// Ends the current line (no-op at the start of a line).
    pub(crate) fn newline(&mut self) {
        if !self.line_start {
            self.buf.push('\n');
            self.line_start = true;
        }
        self.pending_space = false;
    }

    /// Requests one space before the next text (dropped at line ends).
    pub(crate) fn space(&mut self) {
        if !self.line_start {
            self.pending_space = true;
        }
    }

    /// Writes `text` after the indent or a pending space.
    pub(crate) fn text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.line_start {
            let width = 2 * self.indent.min(MAX_INDENT);
            self.buf.push_str(&SPACES[..width.min(SPACES.len())]);
        } else if self.pending_space {
            self.buf.push(' ');
        }
        self.buf.push_str(text);
        self.line_start = false;
        self.pending_space = false;
    }

    /// Writes each line of an already printed block at the current indent.
    pub(crate) fn block(&mut self, printed: &str) {
        for line in printed.lines() {
            self.newline();
            if line.is_empty() {
                continue;
            }
            self.text(line);
        }
        self.newline();
    }

    pub(crate) fn finish(self) -> String {
        self.buf
    }
}
