//! Key-value string files: Flutter `.arb`, Java `.properties` and nested
//! JSON message files (i18next, vue-i18n, Chrome `_locales`, …) flattened
//! to dotted keys.

use corvene_models::TableSheet;

use super::Builder;

/// A Flutter Application Resource Bundle: messages with their
/// `@key.description`; `@@locale`-style fields go to a second sheet.
pub fn arb(text: &str) -> Result<Vec<TableSheet>, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Not a readable ARB file: {e}."))?;
    let map = root.as_object().ok_or("Not an ARB file.")?;
    let mut table = Builder::new("Messages", &["Key", "Value", "Description"]);
    let mut meta = Builder::new("Metadata", &["Field", "Value"]);
    for (key, value) in map {
        if let Some(field) = key.strip_prefix("@@") {
            meta.push(vec![field.to_string(), scalar(value)]);
            continue;
        }
        if key.starts_with('@') {
            continue;
        }
        let description = map
            .get(&format!("@{key}"))
            .and_then(|m| m.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        table.push(vec![key.clone(), scalar(value), description]);
    }
    let mut sheets = vec![table.finish()];
    if !meta.is_empty() {
        sheets.push(meta.finish());
    }
    Ok(sheets)
}

fn scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A Java `.properties` file (escapes, `\` line continuations, `=`/`:`
/// or white space separators, `#`/`!` comments).
pub fn properties(text: &str) -> Result<Vec<TableSheet>, String> {
    let mut table = Builder::new("Properties", &["Key", "Value", "Comment"]);
    let mut comment = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            comment.clear();
            continue;
        }
        if let Some(c) = trimmed
            .strip_prefix('#')
            .or_else(|| trimmed.strip_prefix('!'))
        {
            if !comment.is_empty() {
                comment.push('\n');
            }
            comment.push_str(c.trim());
            continue;
        }
        // join continuation lines (an odd number of trailing backslashes)
        let mut logical = trimmed.to_string();
        while logical.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1 {
            logical.pop();
            match lines.next() {
                Some(next) => logical.push_str(next.trim_start()),
                None => break,
            }
        }
        let (key, value) = split_property(&logical);
        table.push(vec![
            unescape(&key),
            unescape(&value),
            std::mem::take(&mut comment),
        ]);
    }
    if table.is_empty() {
        return Err("No properties were found.".into());
    }
    Ok(vec![table.finish()])
}

fn split_property(line: &str) -> (String, String) {
    let mut key = String::new();
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                key.push('\\');
                if let Some(next) = chars.next() {
                    key.push(next);
                }
            }
            '=' | ':' => break,
            c if c.is_whitespace() => {
                // white space, then possibly one separator
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                if matches!(chars.peek(), Some('=') | Some(':')) {
                    chars.next();
                }
                break;
            }
            c => key.push(c),
        }
    }
    let value: String = chars.collect();
    (key, value.trim_start().to_string())
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => {
                        out.push_str("\\u");
                        out.push_str(&hex);
                    }
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// A nested JSON message file: every string leaf under its dotted path
/// (`menu.file.open`); Chrome's `{"key": {"message": "…", "description":
/// "…"}}` shape keeps its description.
pub fn locale_json(text: &str) -> Result<Vec<TableSheet>, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Not readable JSON: {e}."))?;
    if !root.is_object() {
        return Err("Not a message file.".into());
    }
    let mut table = Builder::new("Messages", &["Key", "Value", "Description"]);
    flatten(&root, String::new(), &mut table, 0);
    if table.is_empty() {
        return Err("No messages were found.".into());
    }
    Ok(vec![table.finish()])
}

fn flatten(value: &serde_json::Value, path: String, table: &mut Builder, depth: usize) {
    if depth > 64 {
        return;
    }
    let join = |key: &str| {
        if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        }
    };
    match value {
        serde_json::Value::Object(map) => {
            // Chrome extension messages
            if let Some(serde_json::Value::String(message)) = map.get("message")
                && map
                    .keys()
                    .all(|k| matches!(k.as_str(), "message" | "description" | "placeholders"))
            {
                let description = map
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string();
                table.push(vec![path, message.clone(), description]);
                return;
            }
            for (key, inner) in map {
                flatten(inner, join(key), table, depth + 1);
            }
        }
        serde_json::Value::Array(items) => {
            for (ix, inner) in items.iter().enumerate() {
                flatten(inner, format!("{path}[{ix}]"), table, depth + 1);
            }
        }
        other => table.push(vec![path, scalar(other), String::new()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arb_messages() {
        let json = r#"{"@@locale": "de", "hello": "Hallo {name}", "@hello": {"description": "Greeting"}, "bye": "Tschüss"}"#;
        let sheets = arb(json).unwrap();
        let rows = &sheets[0].rows;
        assert!(rows.contains(&vec![
            "hello".to_string(),
            "Hallo {name}".into(),
            "Greeting".into()
        ]));
        assert!(rows.contains(&vec!["bye".to_string(), "Tschüss".into(), String::new()]));
        assert_eq!(sheets[1].rows[1], vec!["locale", "de"]);
    }

    #[test]
    fn properties_files() {
        let text = "# Window\nwindow.title = Corvene \\u00e9\nlong=first \\\n    second\nkey\\ with\\ spaces:value\n! bang\nempty\n";
        let rows = properties(text).unwrap().remove(0).rows;
        assert_eq!(rows[1], vec!["window.title", "Corvene é", "Window"]);
        assert_eq!(rows[2], vec!["long", "first second", ""]);
        assert_eq!(rows[3], vec!["key with spaces", "value", ""]);
        assert_eq!(rows[4], vec!["empty", "", "bang"]);
    }

    #[test]
    fn nested_json() {
        let json = r#"{"menu": {"file": {"open": "Öffnen", "recent": ["a", "b"]}}, "ext": {"message": "Hi", "description": "d"}}"#;
        let rows = locale_json(json).unwrap().remove(0).rows;
        assert!(rows.contains(&vec![
            "menu.file.open".to_string(),
            "Öffnen".into(),
            String::new()
        ]));
        assert!(rows.contains(&vec![
            "menu.file.recent[1]".to_string(),
            "b".into(),
            String::new()
        ]));
        assert!(rows.contains(&vec!["ext".to_string(), "Hi".into(), "d".into()]));
    }
}
