//! Comparing text without its whitespace.

use crate::Resolution;

/// A line's comparison key with whitespace ignored: runs of spaces become
/// one, both ends are trimmed and the terminator dropped.
pub fn line_key(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for word in line.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// A whole text's key with whitespace ignored, blank lines included.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// A resolution for a conflict that only whitespace makes: the sides are
/// the same, or one side only re-spaced the base and the other's change
/// wins.
pub fn auto_resolve(base: &str, ours: &str, theirs: &str) -> Option<(Resolution, String)> {
    let (nb, no, nt) = (normalize(base), normalize(ours), normalize(theirs));
    if no == nt {
        return Some((Resolution::AutoWhitespace, ours.to_owned()));
    }
    if no == nb {
        return Some((Resolution::AutoWhitespace, theirs.to_owned()));
    }
    if nt == nb {
        return Some((Resolution::AutoWhitespace, ours.to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_spacing() {
        assert_eq!(line_key("  a \t b\r\n"), "a b");
        assert_eq!(normalize("a\n\n  b\n"), "a b");
    }

    #[test]
    fn same_sides_but_spacing() {
        assert_eq!(
            auto_resolve("x\n", "f(a, b)\n", "f(a,  b)\n").map(|r| r.1),
            Some("f(a, b)\n".to_owned())
        );
    }

    #[test]
    fn respaced_side_yields_to_the_real_change() {
        assert_eq!(
            auto_resolve("f(a)\n", "  f(a)\n", "g(a)\n").map(|r| r.1),
            Some("g(a)\n".to_owned())
        );
        assert_eq!(auto_resolve("f(a)\n", "h(a)\n", "g(a)\n"), None);
    }
}
