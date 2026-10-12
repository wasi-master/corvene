//! gettext catalogs: `.po`/`.pot` source files and compiled `.mo` files.
//! The header entry (`msgid ""`) becomes a sheet of its own (one row per
//! field), the messages another: source, context, plural source, each
//! plural form's translation, flags (`fuzzy`) and comments.

use corvene_models::TableSheet;

use super::Builder;

#[derive(Default, Debug)]
struct Entry {
    context: String,
    id: String,
    plural: String,
    translations: Vec<String>,
    flags: String,
    comment: String,
    obsolete: bool,
}

fn sheets(entries: Vec<Entry>) -> Result<Vec<TableSheet>, String> {
    let mut sheets = Vec::new();
    let forms = entries
        .iter()
        .map(|e| e.translations.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let mut header = vec![
        "Source".to_string(),
        "Context".to_string(),
        "Plural".to_string(),
    ];
    if forms == 1 {
        header.push("Translation".into());
    } else {
        for n in 0..forms {
            header.push(format!("Translation [{n}]"));
        }
    }
    header.push("Flags".into());
    header.push("Comment".into());
    let mut messages = Builder::with_header("Messages", header);
    let mut meta = Builder::new("Header", &["Field", "Value"]);
    for entry in entries {
        if entry.id.is_empty() && entry.context.is_empty() {
            for line in entry
                .translations
                .first()
                .map(String::as_str)
                .unwrap_or("")
                .lines()
            {
                if let Some((k, v)) = line.split_once(':') {
                    meta.push(vec![k.trim().to_string(), v.trim().to_string()]);
                }
            }
            continue;
        }
        let mut row = vec![entry.id, entry.context, entry.plural];
        let mut translations = entry.translations;
        translations.resize(forms, String::new());
        row.extend(translations);
        let mut flags = entry.flags;
        if entry.obsolete {
            if !flags.is_empty() {
                flags.push_str(", ");
            }
            flags.push_str("obsolete");
        }
        row.push(flags);
        row.push(entry.comment);
        messages.push(row);
    }
    if !meta.is_empty() {
        sheets.push(meta.finish());
    }
    if !messages.is_empty() || sheets.is_empty() {
        sheets.insert(0, messages.finish());
    }
    Ok(sheets)
}

/// A C-style quoted string's contents (the quotes taken off).
fn unquote(s: &str) -> Option<String> {
    let inner = s.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('a') => out.push('\u{7}'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('v') => out.push('\u{b}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    Some(out)
}

/// Parses a `.po` or `.pot` file.
pub fn po(text: &str) -> Result<Vec<TableSheet>, String> {
    #[derive(PartialEq, Clone, Copy)]
    enum Field {
        None,
        Context,
        Id,
        Plural,
        Str(usize),
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut cur = Entry::default();
    let mut field = Field::None;
    let mut started = false;
    let mut seen_keyword = false;
    let finish = |cur: &mut Entry, started: &mut bool, entries: &mut Vec<Entry>| {
        if *started {
            entries.push(std::mem::take(cur));
        }
        *started = false;
    };
    for raw in text.lines() {
        let mut line = raw.trim();
        let mut obsolete = false;
        if let Some(rest) = line.strip_prefix("#~") {
            obsolete = true;
            line = rest.trim();
            if line.starts_with('|') {
                continue;
            }
        } else if let Some(rest) = line.strip_prefix('#') {
            // a comment starts a new entry when one is complete
            if field != Field::None && matches!(field, Field::Str(_)) {
                finish(&mut cur, &mut started, &mut entries);
                field = Field::None;
            }
            if let Some(flags) = rest.strip_prefix(',') {
                cur.flags = flags
                    .split(',')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
            } else if let Some(c) = rest.strip_prefix('.') {
                if !cur.comment.is_empty() {
                    cur.comment.push('\n');
                }
                cur.comment.push_str(c.trim());
            } else if rest.starts_with(' ') || rest.is_empty() {
                let c = rest.trim();
                if !c.is_empty() {
                    if !cur.comment.is_empty() {
                        cur.comment.push('\n');
                    }
                    cur.comment.push_str(c);
                }
            }
            continue;
        }
        if line.is_empty() {
            if matches!(field, Field::Str(_)) {
                finish(&mut cur, &mut started, &mut entries);
                field = Field::None;
            }
            continue;
        }
        let (keyword, value) = match line.find('"') {
            Some(0) => ("", line),
            Some(ix) => (line[..ix].trim(), &line[ix..]),
            None => continue,
        };
        let value = unquote(value).unwrap_or_default();
        if !keyword.is_empty() {
            seen_keyword = true;
        }
        let next = match keyword {
            "" => field,
            "msgctxt" => Field::Context,
            "msgid" => Field::Id,
            "msgid_plural" => Field::Plural,
            "msgstr" => Field::Str(0),
            k if k.starts_with("msgstr[") => {
                let n = k[7..].trim_end_matches(']').parse().unwrap_or(0);
                Field::Str(n.min(16))
            }
            _ => continue,
        };
        // a new msgctxt or msgid after a translation starts the next entry
        if matches!(next, Field::Context | Field::Id)
            && (matches!(field, Field::Str(_)) || (next == Field::Id && field == Field::Id))
        {
            finish(&mut cur, &mut started, &mut entries);
        }
        if !keyword.is_empty() {
            started = true;
            field = next;
            if obsolete {
                cur.obsolete = true;
            }
        }
        match field {
            Field::None => {}
            Field::Context => cur.context.push_str(&value),
            Field::Id => cur.id.push_str(&value),
            Field::Plural => cur.plural.push_str(&value),
            Field::Str(n) => {
                if cur.translations.len() <= n {
                    cur.translations.resize(n + 1, String::new());
                }
                cur.translations[n].push_str(&value);
            }
        }
    }
    finish(&mut cur, &mut started, &mut entries);
    if !seen_keyword {
        return Err("Not a gettext catalog.".into());
    }
    sheets(entries)
}

/// Parses a compiled `.mo` file.
pub fn mo(bytes: &[u8]) -> Result<Vec<TableSheet>, String> {
    let magic = bytes.get(..4).ok_or("Not a compiled gettext catalog.")?;
    let le = match magic {
        [0xde, 0x12, 0x04, 0x95] => true,
        [0x95, 0x04, 0x12, 0xde] => false,
        _ => return Err("Not a compiled gettext catalog.".into()),
    };
    let word = |at: usize| -> Result<usize, String> {
        let b = bytes.get(at..at + 4).ok_or("The catalog ends early.")?;
        let b: [u8; 4] = b.try_into().map_err(|_| "The catalog ends early.")?;
        Ok(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        } as usize)
    };
    let count = word(8)?;
    let originals = word(12)?;
    let translations = word(16)?;
    if count > bytes.len() / 8 {
        return Err("Not a compiled gettext catalog (bad count).".into());
    }
    let string = |table: usize, ix: usize| -> Result<&[u8], String> {
        let len = word(table + ix * 8)?;
        let at = word(table + ix * 8 + 4)?;
        bytes
            .get(at..at.checked_add(len).ok_or("Bad string.")?)
            .ok_or_else(|| "A string lies outside the catalog.".to_string())
    };
    let mut entries = Vec::with_capacity(count.min(super::MAX_ROWS + 1));
    for ix in 0..count.min(super::MAX_ROWS + 1) {
        let original = String::from_utf8_lossy(string(originals, ix)?).into_owned();
        let translated = String::from_utf8_lossy(string(translations, ix)?).into_owned();
        let (context, rest) = match original.split_once('\u{4}') {
            Some((c, r)) => (c.to_string(), r.to_string()),
            None => (String::new(), original),
        };
        let (id, plural) = match rest.split_once('\0') {
            Some((i, p)) => (i.to_string(), p.to_string()),
            None => (rest, String::new()),
        };
        entries.push(Entry {
            context,
            id,
            plural,
            translations: translated.split('\0').map(str::to_string).collect(),
            ..Entry::default()
        });
    }
    sheets(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PO: &str = r#"# Translators
msgid ""
msgstr ""
"Language: de\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"

#. greeting on the start page
#: src/app.c:12
msgid "Hello"
msgstr "Hallo"

#, fuzzy
msgctxt "menu"
msgid "Open"
msgstr "Öffnen"

msgid "one file"
msgid_plural "%d files"
msgstr[0] "eine Datei"
msgstr[1] "%d Dateien"

msgid "Long"
msgstr ""
"first line\n"
"second"

#~ msgid "Gone"
#~ msgstr "Weg"
"#;

    #[test]
    fn po_messages_and_header() {
        let sheets = po(PO).unwrap();
        let messages = &sheets[0];
        assert_eq!(
            messages.rows[0],
            vec![
                "Source",
                "Context",
                "Plural",
                "Translation [0]",
                "Translation [1]",
                "Flags",
                "Comment"
            ]
        );
        assert_eq!(
            messages.rows[1],
            vec![
                "Hello",
                "",
                "",
                "Hallo",
                "",
                "",
                "greeting on the start page"
            ]
        );
        assert_eq!(
            messages.rows[2],
            vec!["Open", "menu", "", "Öffnen", "", "fuzzy", ""]
        );
        assert_eq!(
            messages.rows[3],
            vec![
                "one file",
                "",
                "%d files",
                "eine Datei",
                "%d Dateien",
                "",
                ""
            ]
        );
        assert_eq!(messages.rows[4][3], "first line\nsecond");
        assert_eq!(messages.rows[5][0], "Gone");
        assert_eq!(messages.rows[5][5], "obsolete");
        let header = &sheets[1];
        assert_eq!(header.name, "Header");
        assert_eq!(header.rows[1], vec!["Language", "de"]);
    }

    #[test]
    fn mo_round_trip() {
        // two messages: "" (header) and "ctx\x04Open" -> "Öffnen"
        let originals: Vec<&[u8]> = vec![b"", b"menu\x04Open"];
        let translated: Vec<&[u8]> = vec![b"Language: de\n", "Öffnen".as_bytes()];
        let mut bytes = vec![0xde, 0x12, 0x04, 0x95];
        let n = originals.len() as u32;
        let head = 28u32;
        let otab = head;
        let ttab = otab + n * 8;
        let mut data_at = ttab + n * 8;
        for v in [0u32, n, otab, ttab, 0, 0] {
            bytes.extend(v.to_le_bytes());
        }
        let mut table = Vec::new();
        let mut data = Vec::new();
        for s in originals.iter().chain(&translated) {
            table.extend((s.len() as u32).to_le_bytes());
            table.extend(data_at.to_le_bytes());
            data.extend(*s);
            data.push(0);
            data_at += s.len() as u32 + 1;
        }
        bytes.extend(table);
        bytes.extend(data);
        let sheets = mo(&bytes).unwrap();
        assert_eq!(sheets[0].rows[1], vec!["Open", "menu", "Öffnen"]);
        assert_eq!(sheets[1].rows[1], vec!["Language", "de"]);
        assert!(mo(b"nope").is_err());
    }
}
