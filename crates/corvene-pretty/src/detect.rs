//! Minified file detection by name (`*.min.*`) and by line shape.

use crate::Language;

/// Lines longer than this count as machine compacted.
const LONG_LINE: usize = 500;

/// Whether `path`'s file name has a `.min` segment before its extension
/// (`app.min.js`, `style.min.css`, `bundle.min.json`), ignoring case.
pub fn is_minified_name(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((stem, _ext)) = name.rsplit_once('.') else {
        return false;
    };
    let lower = stem.to_ascii_lowercase();
    lower.ends_with(".min") || lower.ends_with("-min") || lower.contains(".min.")
}

/// Whether `text` is clearly machine compacted: most of its bytes sit on
/// very long lines and there are few lines or many long ones, or its lines
/// are very long on average. A hand formatted file with one long string
/// line among many normal ones is not minified.
pub fn is_minified(language: Language, text: &str) -> bool {
    let _ = language;
    let mut total = 0usize;
    let mut lines = 0usize;
    let mut long_lines = 0usize;
    let mut long_bytes = 0usize;
    for line in text.split('\n') {
        let len = line.trim().len();
        if len == 0 {
            continue;
        }
        lines += 1;
        total += len;
        if len > LONG_LINE {
            long_lines += 1;
            long_bytes += len;
        }
    }
    if lines == 0 {
        return false;
    }
    if long_lines > 0 && long_bytes * 2 > total && (lines <= 20 || long_lines * 5 >= lines) {
        return true;
    }
    total > 2000 && total / lines > 300
}
