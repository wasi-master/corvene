//! Small helpers shared by the text conversions.

/// `text` cut to at most `max` bytes (at a line end where possible), with a
/// note saying so.
pub(crate) fn cap(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').map_or(end, |ix| ix + 1);
    let mut text = text[..end].to_string();
    text.push_str("… (the rest is left out: too long to compare)\n");
    text
}

/// `bytes` as a string literal: printable ASCII and UTF-8 kept, the rest
/// escaped.
pub(crate) fn quote(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Short bytes as hex (`0a1b…`), long ones cut with their length.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const SHOWN: usize = 64;
    let mut out: String = bytes
        .iter()
        .take(SHOWN)
        .map(|b| format!("{b:02x}"))
        .collect();
    if bytes.len() > SHOWN {
        out.push_str(&format!("… ({} bytes)", bytes.len()));
    }
    out
}

/// A byte count with thousands separators (`12,345`).
pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, ch) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_cuts_at_a_line() {
        let text = "aaaa\nbbbb\ncccc\n".to_string();
        let cut = cap(text, 12);
        assert!(cut.starts_with("aaaa\nbbbb\n…"));
        assert_eq!(cap("ab".into(), 12), "ab");
    }

    #[test]
    fn thousands_groups() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(1234), "1,234");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
