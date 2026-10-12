//! XLIFF 1.2 (`<trans-unit>`) and 2.0 (`<unit>`/`<segment>`) files, as
//! exported by Xcode, Angular, Okapi and most translation tools: a row
//! per unit with its source, target, state and notes, a sheet per
//! `<file>` when there are several.

use corvene_models::TableSheet;

use super::Builder;

/// The text of an element, inline placeholders shown as `{id}`.
pub(crate) fn inline_text(node: roxmltree::Node<'_, '_>) -> String {
    let mut out = String::new();
    for child in node.children() {
        if child.is_text() {
            out.push_str(child.text().unwrap_or(""));
        } else if child.is_element() {
            match child.tag_name().name() {
                // placeholders with no content of their own
                "x" | "bx" | "ex" | "ph" | "sc" | "ec" => {
                    let inner = inline_text(child);
                    if !inner.is_empty() {
                        out.push_str(&inner);
                    } else if let Some(shown) = child
                        .attribute("equiv-text")
                        .or_else(|| child.attribute("disp"))
                    {
                        out.push_str(shown);
                    } else {
                        out.push_str(&format!("{{{}}}", child.attribute("id").unwrap_or("")));
                    }
                }
                _ => out.push_str(&inline_text(child)),
            }
        }
    }
    out
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Parses an XLIFF file.
pub fn xliff(text: &str) -> Result<Vec<TableSheet>, String> {
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Not a readable XLIFF file: {e}."))?;
    let root = doc.root_element();
    if root.tag_name().name() != "xliff" {
        return Err("Not an XLIFF file.".into());
    }
    let v2 = root
        .attribute("version")
        .is_some_and(|v| v.starts_with('2'));
    let files: Vec<_> = root
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "file")
        .collect();
    let mut sheets = Vec::new();
    for file in &files {
        let name = file
            .attribute("original")
            .or_else(|| file.attribute("id"))
            .unwrap_or("Strings");
        let target_lang = file
            .attribute("target-language")
            .or_else(|| root.attribute("trgLang"))
            .unwrap_or("");
        let target_title = if target_lang.is_empty() {
            "Target".to_string()
        } else {
            format!("Target ({target_lang})")
        };
        let mut table = Builder::with_header(
            name,
            vec![
                "ID".into(),
                "Source".into(),
                target_title,
                "State".into(),
                "Note".into(),
            ],
        );
        let units = file.descendants().filter(|n| {
            n.is_element()
                && if v2 {
                    n.tag_name().name() == "unit"
                } else {
                    n.tag_name().name() == "trans-unit"
                }
        });
        for unit in units {
            let id = unit
                .attribute("resname")
                .or_else(|| unit.attribute("name"))
                .or_else(|| unit.attribute("id"))
                .unwrap_or("")
                .to_string();
            let notes: Vec<String> = unit
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "note")
                .map(inline_text)
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            if v2 {
                let segments: Vec<_> = unit
                    .children()
                    .filter(|c| {
                        c.is_element() && matches!(c.tag_name().name(), "segment" | "ignorable")
                    })
                    .collect();
                let source: String = segments
                    .iter()
                    .filter_map(|s| child(*s, "source"))
                    .map(inline_text)
                    .collect();
                let target: String = segments
                    .iter()
                    .filter_map(|s| child(*s, "target"))
                    .map(inline_text)
                    .collect();
                let state = segments
                    .iter()
                    .find_map(|s| s.attribute("state"))
                    .unwrap_or("")
                    .to_string();
                table.push(vec![id, source, target, state, notes.join("\n")]);
            } else {
                let source = child(unit, "source").map(inline_text).unwrap_or_default();
                let target_node = child(unit, "target");
                let target = target_node.map(inline_text).unwrap_or_default();
                let mut state = target_node
                    .and_then(|t| t.attribute("state"))
                    .unwrap_or("")
                    .to_string();
                if unit.attribute("translate") == Some("no") {
                    state = "not translated".into();
                }
                table.push(vec![id, source, target, state, notes.join("\n")]);
            }
        }
        if !table.is_empty() {
            sheets.push(table.finish());
        }
    }
    if sheets.is_empty() {
        return Err("No translation units were found.".into());
    }
    Ok(sheets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xliff_12() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<xliff xmlns="urn:oasis:names:tc:xliff:document:1.2" version="1.2">
  <file original="Main.strings" source-language="en" target-language="de" datatype="plaintext">
    <body>
      <trans-unit id="greet">
        <source>Hello <x id="1" equiv-text="{name}"/>!</source>
        <target state="translated">Hallo <x id="1" equiv-text="{name}"/>!</target>
        <note>On the start page</note>
      </trans-unit>
      <trans-unit id="bye"><source>Bye</source></trans-unit>
    </body>
  </file>
</xliff>"#;
        let sheets = xliff(xml).unwrap();
        assert_eq!(sheets[0].name, "Main.strings");
        assert_eq!(
            sheets[0].rows[0],
            vec!["ID", "Source", "Target (de)", "State", "Note"]
        );
        assert_eq!(
            sheets[0].rows[1],
            vec![
                "greet",
                "Hello {name}!",
                "Hallo {name}!",
                "translated",
                "On the start page"
            ]
        );
        assert_eq!(sheets[0].rows[2], vec!["bye", "Bye", "", "", ""]);
    }

    #[test]
    fn xliff_20() {
        let xml = r#"<xliff xmlns="urn:oasis:names:tc:xliff:document:2.0" version="2.0" srcLang="en" trgLang="fr">
  <file id="f1"><unit id="u1"><notes><note>n</note></notes>
    <segment state="final"><source>Save</source><target>Enregistrer</target></segment></unit></file>
</xliff>"#;
        let sheets = xliff(xml).unwrap();
        assert_eq!(
            sheets[0].rows[0],
            vec!["ID", "Source", "Target (fr)", "State", "Note"]
        );
        assert_eq!(
            sheets[0].rows[1],
            vec!["u1", "Save", "Enregistrer", "final", "n"]
        );
    }
}
