//! Apple string tables: `.strings` (text in UTF-8 or UTF-16, or a binary
//! property list), `.stringsdict` (plural rules in a property list) and
//! Xcode string catalogs (`.xcstrings`, JSON with every language).

use corvene_models::TableSheet;

use super::Builder;

/// A `.strings` file: key, value and the comment above each pair.
pub fn strings(bytes: &[u8]) -> Result<Vec<TableSheet>, String> {
    let mut table = Builder::new("Strings", &["Key", "Value", "Comment"]);
    if bytes.starts_with(b"bplist00") {
        let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
            .map_err(|e| format!("Not a readable strings file: {e}."))?;
        let dict = value.as_dictionary().ok_or("Not a strings file.")?;
        for (key, value) in dict {
            table.push(vec![key.clone(), plist_text(value), String::new()]);
        }
        return Ok(vec![table.finish()]);
    }
    let text = super::text(bytes)?;
    let mut p = Parser {
        chars: text.chars().collect(),
        pos: 0,
    };
    let mut comment = String::new();
    loop {
        if let Some(c) = p.skip_space_and_comments() {
            comment = c;
        }
        if p.pos >= p.chars.len() {
            break;
        }
        let key = p.string().ok_or("Not a strings file (expected a key).")?;
        p.skip_space_and_comments();
        if !p.eat('=') {
            // `"key";` is a key that is its own value
            if p.eat(';') {
                table.push(vec![key.clone(), key, std::mem::take(&mut comment)]);
                continue;
            }
            return Err("Not a strings file (expected `=`).".into());
        }
        p.skip_space_and_comments();
        let value = p.string().ok_or("Not a strings file (expected a value).")?;
        p.skip_space_and_comments();
        if !p.eat(';') {
            return Err("Not a strings file (expected `;`).".into());
        }
        table.push(vec![key, value, std::mem::take(&mut comment)]);
    }
    if table.is_empty() && !text.trim().is_empty() && !text.contains("/*") && !text.contains("//") {
        return Err("Not a strings file.".into());
    }
    Ok(vec![table.finish()])
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Skips white space and comments; returns the last comment's text.
    fn skip_space_and_comments(&mut self) -> Option<String> {
        let mut last = None;
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.pos += 1;
            }
            if self.peek() == Some('/') && self.chars.get(self.pos + 1) == Some(&'*') {
                let start = self.pos + 2;
                let mut end = start;
                while end + 1 < self.chars.len()
                    && !(self.chars[end] == '*' && self.chars[end + 1] == '/')
                {
                    end += 1;
                }
                last = Some(
                    self.chars[start..end.min(self.chars.len())]
                        .iter()
                        .collect::<String>()
                        .trim()
                        .to_string(),
                );
                self.pos = (end + 2).min(self.chars.len());
            } else if self.peek() == Some('/') && self.chars.get(self.pos + 1) == Some(&'/') {
                let start = self.pos + 2;
                let mut end = start;
                while end < self.chars.len() && self.chars[end] != '\n' {
                    end += 1;
                }
                last = Some(
                    self.chars[start..end]
                        .iter()
                        .collect::<String>()
                        .trim()
                        .to_string(),
                );
                self.pos = end;
            } else {
                return last;
            }
        }
    }

    /// A quoted string (with `\n`, `\"`, `\U1234` escapes) or an unquoted
    /// word.
    fn string(&mut self) -> Option<String> {
        if self.eat('"') {
            let mut out = String::new();
            loop {
                let ch = self.peek()?;
                self.pos += 1;
                match ch {
                    '"' => return Some(out),
                    '\\' => {
                        let esc = self.peek()?;
                        self.pos += 1;
                        match esc {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'r' => out.push('\r'),
                            'U' | 'u' => {
                                let hex: String = self.chars[self.pos..]
                                    .iter()
                                    .take(4)
                                    .take_while(|c| c.is_ascii_hexdigit())
                                    .collect();
                                self.pos += hex.len();
                                if let Some(c) =
                                    u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                                {
                                    out.push(c);
                                }
                            }
                            other => out.push(other),
                        }
                    }
                    other => out.push(other),
                }
            }
        }
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_alphanumeric() || "_.$:/-".contains(c))
        {
            self.pos += 1;
        }
        (self.pos > start).then(|| self.chars[start..self.pos].iter().collect())
    }
}

/// A property list value as a cell.
fn plist_text(value: &plist::Value) -> String {
    match value {
        plist::Value::String(s) => s.clone(),
        plist::Value::Integer(i) => i.to_string(),
        plist::Value::Real(r) => r.to_string(),
        plist::Value::Boolean(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

/// A `.stringsdict` file: each key's format and plural rules as rows
/// (`files › count › one`).
pub fn stringsdict(bytes: &[u8]) -> Result<Vec<TableSheet>, String> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Not a readable stringsdict file: {e}."))?;
    let dict = value.as_dictionary().ok_or("Not a stringsdict file.")?;
    let mut table = Builder::new("Plurals", &["Key", "Value"]);
    for (key, entry) in dict {
        let Some(entry) = entry.as_dictionary() else {
            table.push(vec![key.clone(), plist_text(entry)]);
            continue;
        };
        if let Some(format) = entry.get("NSStringLocalizedFormatKey") {
            table.push(vec![key.clone(), plist_text(format)]);
        }
        for (var, rules) in entry {
            if var == "NSStringLocalizedFormatKey" {
                continue;
            }
            let Some(rules) = rules.as_dictionary() else {
                continue;
            };
            for (rule, text) in rules {
                if rule.starts_with("NSStringFormat") {
                    continue;
                }
                table.push(vec![format!("{key} › {var} › {rule}"), plist_text(text)]);
            }
        }
    }
    Ok(vec![table.finish()])
}

/// An Xcode string catalog: a row per key (and per plural or device
/// variation), a column per language with the source language first; a
/// value not in the `translated` state carries it (`Hallo [needs_review]`).
pub fn xcstrings(text: &str) -> Result<Vec<TableSheet>, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Not a readable string catalog: {e}."))?;
    let source = root
        .get("sourceLanguage")
        .and_then(|v| v.as_str())
        .unwrap_or("en")
        .to_string();
    let strings = root
        .get("strings")
        .and_then(|v| v.as_object())
        .ok_or("Not a string catalog (no strings).")?;
    let mut languages: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in strings.values() {
        if let Some(locs) = entry.get("localizations").and_then(|v| v.as_object()) {
            languages.extend(locs.keys().cloned());
        }
    }
    languages.remove(&source);
    let mut columns = vec![source.clone()];
    columns.extend(languages);
    let mut header = vec!["Key".to_string(), "Comment".to_string()];
    header.extend(columns.iter().cloned());
    let mut table = Builder::with_header("Strings", header);
    // sorted as Xcode writes them (and whatever order the JSON map keeps)
    let mut entries: Vec<_> = strings.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (key, entry) in entries {
        let comment = entry
            .get("comment")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut flags = Vec::new();
        if entry.get("shouldTranslate").and_then(|v| v.as_bool()) == Some(false) {
            flags.push("not translated");
        }
        if let Some(state) = entry.get("extractionState").and_then(|v| v.as_str())
            && state == "stale"
        {
            flags.push("stale");
        }
        let comment = match (comment.is_empty(), flags.is_empty()) {
            (_, true) => comment,
            (true, false) => format!("({})", flags.join(", ")),
            (false, false) => format!("{comment} ({})", flags.join(", ")),
        };
        // every variation path with each language's value
        let mut rows: Vec<(String, Vec<String>)> = Vec::new();
        let locs = entry.get("localizations").and_then(|v| v.as_object());
        for (ci, lang) in columns.iter().enumerate() {
            let Some(loc) = locs.and_then(|l| l.get(lang)) else {
                continue;
            };
            let mut found = Vec::new();
            units(loc, String::new(), &mut found);
            for (path, value) in found {
                let row = match rows.iter_mut().find(|(p, _)| *p == path) {
                    Some(row) => row,
                    None => {
                        rows.push((path.clone(), vec![String::new(); columns.len()]));
                        rows.last_mut().expect("just pushed")
                    }
                };
                row.1[ci] = value;
            }
        }
        if rows.is_empty() {
            rows.push((String::new(), vec![String::new(); columns.len()]));
        }
        for (path, mut values) in rows {
            // the key is the source text when the source has no unit
            if path.is_empty() && values[0].is_empty() {
                values[0] = key.clone();
            }
            let shown_key = if path.is_empty() {
                key.clone()
            } else {
                format!("{key} ({path})")
            };
            let mut row = vec![shown_key, comment.clone()];
            row.extend(values);
            table.push(row);
        }
    }
    Ok(vec![table.finish()])
}

/// The string units under a localization or variation, with their
/// variation path (`plural: one`, `device: iphone, plural: other`).
fn units(node: &serde_json::Value, path: String, out: &mut Vec<(String, String)>) {
    if let Some(unit) = node.get("stringUnit") {
        let value = unit.get("value").and_then(|v| v.as_str()).unwrap_or("");
        let state = unit
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("translated");
        let value = if state == "translated" {
            value.to_string()
        } else {
            format!("{value} [{state}]")
        };
        out.push((path.clone(), value));
    }
    if let Some(vars) = node.get("variations").and_then(|v| v.as_object()) {
        for (kind, cases) in vars {
            let Some(cases) = cases.as_object() else {
                continue;
            };
            for (case, inner) in cases {
                let step = format!("{kind}: {case}");
                let next = if path.is_empty() {
                    step
                } else {
                    format!("{path}, {step}")
                };
                units(inner, next, out);
            }
        }
    }
    if let Some(subs) = node.get("substitutions").and_then(|v| v.as_object()) {
        for (name, sub) in subs {
            let next = if path.is_empty() {
                format!("%#@{name}@")
            } else {
                format!("{path}, %#@{name}@")
            };
            units(sub, next, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_files() {
        let text = "/* Title of the window */\n\"window.title\" = \"Corvene\";\n\n// short\nkey2 = \"Line\\nTwo \\\"q\\\" \\U00e9\";\n";
        let sheets = strings(text.as_bytes()).unwrap();
        assert_eq!(sheets[0].rows[0], vec!["Key", "Value", "Comment"]);
        assert_eq!(
            sheets[0].rows[1],
            vec!["window.title", "Corvene", "Title of the window"]
        );
        assert_eq!(
            sheets[0].rows[2],
            vec!["key2", "Line\nTwo \"q\" é", "short"]
        );
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("\"a\" = \"b\";".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(strings(&utf16).unwrap()[0].rows[1], vec!["a", "b"]);
        assert!(strings(b"{ \"json\": 1 }").is_err());
    }

    #[test]
    fn stringsdict_rules() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>files</key><dict>
<key>NSStringLocalizedFormatKey</key><string>%#@count@</string>
<key>count</key><dict><key>NSStringFormatSpecTypeKey</key><string>NSStringPluralRuleType</string>
<key>one</key><string>One file</string><key>other</key><string>%d files</string></dict>
</dict></dict></plist>"#;
        let sheets = stringsdict(xml.as_bytes()).unwrap();
        assert_eq!(sheets[0].rows[1], vec!["files", "%#@count@"]);
        assert_eq!(sheets[0].rows[2], vec!["files › count › one", "One file"]);
        assert_eq!(sheets[0].rows[3], vec!["files › count › other", "%d files"]);
    }

    #[test]
    fn catalogs_have_a_column_per_language() {
        let json = r#"{
  "sourceLanguage" : "en",
  "strings" : {
    "Hello" : {
      "comment" : "Greeting",
      "localizations" : {
        "de" : { "stringUnit" : { "state" : "translated", "value" : "Hallo" } },
        "fr" : { "stringUnit" : { "state" : "needs_review", "value" : "Bonjour" } }
      }
    },
    "%lld files" : {
      "localizations" : {
        "en" : { "variations" : { "plural" : {
          "one" : { "stringUnit" : { "state" : "translated", "value" : "%lld file" } },
          "other" : { "stringUnit" : { "state" : "translated", "value" : "%lld files" } } } } }
      }
    }
  },
  "version" : "1.0"
}"#;
        let sheets = xcstrings(json).unwrap();
        let rows = &sheets[0].rows;
        assert_eq!(rows[0], vec!["Key", "Comment", "en", "de", "fr"]);
        assert_eq!(
            rows[1],
            vec!["%lld files (plural: one)", "", "%lld file", "", ""]
        );
        assert_eq!(
            rows[2],
            vec!["%lld files (plural: other)", "", "%lld files", "", ""]
        );
        assert_eq!(
            rows[3],
            vec![
                "Hello",
                "Greeting",
                "Hello",
                "Hallo",
                "Bonjour [needs_review]"
            ]
        );
    }
}
