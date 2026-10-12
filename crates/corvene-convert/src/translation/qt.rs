//! Qt Linguist files: `.ts` sources (XML; the extension is shared with
//! TypeScript, so the bytes decide) and compiled `.qm` files.

use corvene_models::TableSheet;

use super::Builder;
use super::xliff::inline_text;

/// Whether `sample` is the start of a Qt `.ts` file rather than TypeScript.
pub fn is_ts(sample: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&sample[..sample.len().min(1024)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    (head.starts_with("<?xml") && (head.contains("<!DOCTYPE TS") || head.contains("<TS")))
        || head.starts_with("<!DOCTYPE TS")
        || head.starts_with("<TS")
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
}

/// A `.ts` file: a row per message (source, context, translation, state,
/// comments); numerus forms joined one per line.
pub fn ts(text: &str) -> Result<Vec<TableSheet>, String> {
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Not a readable Qt translation file: {e}."))?;
    let root = doc.root_element();
    if root.tag_name().name() != "TS" {
        return Err("Not a Qt translation file.".into());
    }
    let language = root.attribute("language").unwrap_or("");
    let title = if language.is_empty() {
        "Translation".to_string()
    } else {
        format!("Translation ({language})")
    };
    let mut table = Builder::with_header(
        "Messages",
        vec![
            "Source".into(),
            "Context".into(),
            title,
            "State".into(),
            "Comment".into(),
        ],
    );
    for context in root
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "context")
    {
        let name = child(context, "name").map(inline_text).unwrap_or_default();
        for message in context
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "message")
        {
            let source = child(message, "source")
                .map(inline_text)
                .unwrap_or_default();
            let translation = child(message, "translation");
            let text = match translation {
                Some(t) if message.attribute("numerus") == Some("yes") => t
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "numerusform")
                    .map(inline_text)
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(t) => inline_text(t),
                None => String::new(),
            };
            let state = translation
                .and_then(|t| t.attribute("type"))
                .unwrap_or("")
                .to_string();
            let comment = ["comment", "extracomment", "translatorcomment"]
                .iter()
                .filter_map(|tag| child(message, tag).map(inline_text))
                .filter(|c| !c.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            table.push(vec![source, name.clone(), text, state, comment]);
        }
    }
    Ok(vec![table.finish()])
}

const QM_MAGIC: [u8; 16] = [
    0x3c, 0xb8, 0x64, 0x18, 0xca, 0xef, 0x9c, 0x95, 0xcd, 0x21, 0x1c, 0xbf, 0x60, 0xa1, 0xbd, 0xdd,
];

/// A compiled `.qm` file. Its messages keep their source text only when
/// `lrelease` was told to; otherwise rows are numbered.
pub fn qm(bytes: &[u8]) -> Result<Vec<TableSheet>, String> {
    if !bytes.starts_with(&QM_MAGIC) {
        return Err("Not a compiled Qt translation.".into());
    }
    let mut pos = QM_MAGIC.len();
    let be32 = |at: usize| -> Option<u32> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut messages: Option<&[u8]> = None;
    let mut language = String::new();
    while pos + 5 <= bytes.len() {
        let tag = bytes[pos];
        let len = be32(pos + 1).ok_or("The file ends early.")? as usize;
        let start = pos + 5;
        let data = bytes
            .get(start..start.checked_add(len).ok_or("Bad section.")?)
            .ok_or("A section runs past the end.")?;
        match tag {
            0x42 => messages = Some(data),
            0xa7 => language = String::from_utf8_lossy(data).into_owned(),
            _ => {}
        }
        pos = start + len;
    }
    let data = messages.ok_or("No messages were found.")?;
    let title = if language.is_empty() {
        "Translation".to_string()
    } else {
        format!("Translation ({language})")
    };
    let mut table = Builder::with_header(
        "Messages",
        vec!["Source".into(), "Context".into(), title, "Comment".into()],
    );
    let mut p = 0;
    let mut n = 0;
    let read_len = |p: usize| -> Option<u32> {
        data.get(p..p + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    while p < data.len() {
        let mut source = String::new();
        let mut context = String::new();
        let mut comment = String::new();
        let mut translations: Vec<String> = Vec::new();
        while let Some(&tag) = data.get(p) {
            p += 1;
            match tag {
                1 => break,
                3 => {
                    let len = read_len(p).ok_or("Bad message.")?;
                    p += 4;
                    if len == u32::MAX {
                        translations.push(String::new());
                        continue;
                    }
                    let b = data.get(p..p + len as usize).ok_or("Bad message.")?;
                    let units: Vec<u16> = b
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect();
                    translations.push(String::from_utf16_lossy(&units));
                    p += len as usize;
                }
                5 => p += 4,
                6..=8 => {
                    let len = read_len(p).ok_or("Bad message.")? as usize;
                    p += 4;
                    let b = data.get(p..p + len).ok_or("Bad message.")?;
                    let text = String::from_utf8_lossy(b)
                        .trim_end_matches('\0')
                        .to_string();
                    match tag {
                        6 => source = text,
                        7 => context = text,
                        _ => comment = text,
                    }
                    p += len;
                }
                // obsolete 16-bit fields and hashes: length-prefixed
                2 | 4 => {
                    let len = read_len(p).ok_or("Bad message.")? as usize;
                    p += 4 + len;
                }
                9 => p += 4,
                _ => return Err("Not a readable compiled Qt translation.".into()),
            }
        }
        n += 1;
        if source.is_empty() {
            source = format!("#{n}");
        }
        table.push(vec![source, context, translations.join("\n"), comment]);
    }
    Ok(vec![table.finish()])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE TS>
<TS version="2.1" language="de_DE">
<context>
    <name>MainWindow</name>
    <message>
        <location filename="main.cpp" line="10"/>
        <source>&amp;Open</source>
        <translation>Ö&amp;ffnen</translation>
    </message>
    <message numerus="yes">
        <source>%n file(s)</source>
        <translation type="unfinished">
            <numerusform>%n Datei</numerusform>
            <numerusform>%n Dateien</numerusform>
        </translation>
    </message>
</context>
</TS>"#;

    #[test]
    fn ts_messages() {
        assert!(is_ts(TS.as_bytes()));
        assert!(!is_ts(b"import x from 'y';"));
        let rows = ts(TS).unwrap().remove(0).rows;
        assert_eq!(
            rows[0],
            vec!["Source", "Context", "Translation (de_DE)", "State"]
        );
        assert_eq!(rows[1], vec!["&Open", "MainWindow", "Ö&ffnen", ""]);
        assert_eq!(
            rows[2],
            vec![
                "%n file(s)",
                "MainWindow",
                "%n Datei\n%n Dateien",
                "unfinished"
            ]
        );
    }

    #[test]
    fn qm_messages() {
        let mut msg = Vec::new();
        msg.push(3);
        let t: Vec<u8> = "Öffnen".encode_utf16().flat_map(u16::to_be_bytes).collect();
        msg.extend((t.len() as u32).to_be_bytes());
        msg.extend(t);
        msg.push(6);
        msg.extend(5u32.to_be_bytes());
        msg.extend(b"&Open");
        msg.push(7);
        msg.extend(10u32.to_be_bytes());
        msg.extend(b"MainWindow");
        msg.push(1);
        let mut bytes = QM_MAGIC.to_vec();
        bytes.push(0x42);
        bytes.extend((msg.len() as u32).to_be_bytes());
        bytes.extend(msg);
        let rows = qm(&bytes).unwrap().remove(0).rows;
        assert_eq!(rows[1], vec!["&Open", "MainWindow", "Öffnen"]);
        assert!(qm(b"nope").is_err());
    }
}
