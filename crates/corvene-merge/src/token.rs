//! Word-level merges inside a conflict: when the two sides changed
//! different words of the same lines, a diff3 over tokens keeps both
//! changes (JetBrains' "resolve simple conflicts").

use crate::diff3::diff3_ids;
use crate::text::Interner;
use crate::{ChunkKind, ConflictStyle};

/// Sides larger than this are not tried.
const MAX_BYTES: usize = 64 * 1024;

/// Words, runs of whitespace and single other characters.
pub fn tokens(text: &str) -> Vec<&str> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Newline,
        Other,
    }
    fn class(c: char) -> Class {
        if c == '\n' {
            Class::Newline
        } else if c.is_whitespace() {
            Class::Space
        } else if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else {
            Class::Other
        }
    }
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<Class> = None;
    for (i, c) in text.char_indices() {
        let k = class(c);
        let joins = matches!(
            (&prev, &k),
            (Some(Class::Word), Class::Word) | (Some(Class::Space), Class::Space)
        );
        if !joins && i > start {
            out.push(&text[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Both sides' changes in one text, or `None` when they touch the same
/// words.
pub fn merge(base: &str, ours: &str, theirs: &str) -> Option<String> {
    if [base, ours, theirs].iter().any(|s| s.len() > MAX_BYTES) {
        return None;
    }
    let (bt, ot, tt) = (tokens(base), tokens(ours), tokens(theirs));
    let mut interner = Interner::default();
    let (b, o, t) = (
        interner.ids(bt.iter().copied()),
        interner.ids(ot.iter().copied()),
        interner.ids(tt.iter().copied()),
    );
    let chunks = diff3_ids(&b, &o, &t, ConflictStyle::Diff3);
    if chunks.iter().any(|c| c.kind == ChunkKind::Conflict) {
        return None;
    }
    let mut out = String::with_capacity(ours.len().max(theirs.len()));
    for chunk in chunks {
        let parts = match chunk.kind {
            ChunkKind::TheirsOnly => &tt[chunk.theirs],
            _ => &ot[chunk.ours],
        };
        parts.iter().for_each(|p| out.push_str(p));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_spaces_and_punctuation() {
        assert_eq!(
            tokens("let x_1 = f(a);\n"),
            [
                "let", " ", "x_1", " ", "=", " ", "f", "(", "a", ")", ";", "\n"
            ]
        );
    }

    #[test]
    fn different_words_of_one_line_merge() {
        assert_eq!(
            merge(
                "let total = price * count;\n",
                "let total = cost * count;\n",
                "let total = price * amount;\n"
            )
            .as_deref(),
            Some("let total = cost * amount;\n")
        );
    }

    #[test]
    fn the_same_word_does_not() {
        assert_eq!(merge("a = 1;\n", "a = 2;\n", "a = 3;\n"), None);
    }

    #[test]
    fn deleting_a_line_the_other_side_edited_does_not() {
        assert_eq!(merge("x = 1;\n", "", "x = 2;\n"), None);
    }
}
