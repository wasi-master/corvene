//! Bracket printer for text no language printer accepts (invalid JSON):
//! newline + indent after `{` and `[`, before `}` and `]`, after `,` and
//! `;`. Quoted runs stay verbatim; other whitespace collapses.

use crate::out::Out;

pub(crate) fn pretty(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Out::with_capacity(text.len() + text.len() / 4);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' | b'\'' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(bytes.len());
                out.text(&text[start..i]);
                continue;
            }
            b'{' | b'[' => {
                out.text(&text[i..i + 1]);
                out.indent += 1;
                out.newline();
            }
            b'}' | b']' => {
                out.indent = out.indent.saturating_sub(1);
                out.newline();
                out.text(&text[i..i + 1]);
            }
            b',' | b';' => {
                out.text(&text[i..i + 1]);
                out.newline();
            }
            b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' => out.space(),
            _ => {
                let start = i;
                while i < bytes.len()
                    && !matches!(
                        bytes[i],
                        b'"' | b'\''
                            | b'{'
                            | b'['
                            | b'}'
                            | b']'
                            | b','
                            | b';'
                            | b' '
                            | b'\t'
                            | b'\n'
                            | b'\r'
                            | b'\x0c'
                    )
                {
                    i += 1;
                }
                out.text(&text[start..i]);
                continue;
            }
        }
        i += 1;
    }
    out.finish()
}
