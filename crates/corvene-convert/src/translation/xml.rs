//! Android string resources (`res/values*/strings.xml`: strings, plurals,
//! string arrays) and .NET resource files (`.resx`/`.resw`).

use corvene_models::TableSheet;

use super::Builder;

fn parse(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Not readable XML: {e}."))
}

/// An element's contents as written (inline markup such as `<b>` or
/// `<xliff:g>` kept), with XML character references decoded.
fn inner_markup(text: &str, node: roxmltree::Node<'_, '_>) -> String {
    let (Some(first), Some(last)) = (node.first_child(), node.last_child()) else {
        return String::new();
    };
    let raw = &text[first.range().start..last.range().end];
    decode_entities(raw.trim())
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        // Bytes, not a str slice: byte 10 can fall inside a multi-byte char.
        let Some(semi) = tail.bytes().take(10).position(|b| b == b';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let body = &tail[1..semi];
        let decoded = match body {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => body.strip_prefix('#').and_then(|n| {
                match n.strip_prefix('x') {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => n.parse().ok(),
                }
                .and_then(char::from_u32)
            }),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// An Android `<resources>` file.
pub fn android(text: &str) -> Result<Vec<TableSheet>, String> {
    let doc = parse(text)?;
    let root = doc.root_element();
    if root.tag_name().name() != "resources" {
        return Err("Not an Android resource file.".into());
    }
    let mut table = Builder::new("Strings", &["Name", "Value", "Note"]);
    for node in root.children().filter(|n| n.is_element()) {
        let name = node.attribute("name").unwrap_or("").to_string();
        let mut note = Vec::new();
        if node.attribute("translatable") == Some("false") {
            note.push("not translatable".to_string());
        }
        if let Some(fmt) = node.attribute("formatted") {
            note.push(format!("formatted={fmt}"));
        }
        let note = note.join(", ");
        match node.tag_name().name() {
            "string" => table.push(vec![name, inner_markup(text, node), note]),
            "plurals" => {
                for item in node.children().filter(|n| n.is_element()) {
                    let quantity = item.attribute("quantity").unwrap_or("?");
                    table.push(vec![
                        format!("{name} ({quantity})"),
                        inner_markup(text, item),
                        note.clone(),
                    ]);
                }
            }
            "string-array" | "array" | "integer-array" => {
                for (ix, item) in node.children().filter(|n| n.is_element()).enumerate() {
                    table.push(vec![
                        format!("{name}[{ix}]"),
                        inner_markup(text, item),
                        note.clone(),
                    ]);
                }
            }
            _ => {}
        }
    }
    Ok(vec![table.finish()])
}

/// A .NET `.resx` file: its `<data>` entries (binary resources named by
/// their type).
pub fn resx(text: &str) -> Result<Vec<TableSheet>, String> {
    let doc = parse(text)?;
    let root = doc.root_element();
    if root.tag_name().name() != "root" {
        return Err("Not a .NET resource file.".into());
    }
    let mut table = Builder::new("Resources", &["Name", "Value", "Comment"]);
    for data in root
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "data")
    {
        let name = data.attribute("name").unwrap_or("").to_string();
        let text_of = |tag: &str| {
            data.children()
                .find(|c| c.is_element() && c.tag_name().name() == tag)
                .map(|c| c.text().unwrap_or("").to_string())
                .unwrap_or_default()
        };
        let value = match data
            .attribute("type")
            .or_else(|| data.attribute("mimetype"))
        {
            Some(ty) if !ty.starts_with("System.String") => {
                let ty = ty.split(',').next().unwrap_or(ty);
                format!("({ty}, {} bytes)", text_of("value").trim().len())
            }
            _ => text_of("value"),
        };
        table.push(vec![name, value, text_of("comment")]);
    }
    Ok(vec![table.finish()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_resources() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<resources xmlns:xliff="urn:oasis:names:tc:xliff:document:1.2">
    <string name="app_name" translatable="false">Corvene</string>
    <string name="welcome">Hi <b><xliff:g id="name">%1$s</xliff:g></b> &amp; co</string>
    <plurals name="files">
        <item quantity="one">%d file</item>
        <item quantity="other">%d files</item>
    </plurals>
    <string-array name="days"><item>Mon</item><item>Tue</item></string-array>
</resources>"#;
        let rows = android(xml).unwrap().remove(0).rows;
        assert_eq!(rows[0], vec!["Name", "Value", "Note"]);
        assert_eq!(rows[1], vec!["app_name", "Corvene", "not translatable"]);
        assert_eq!(
            rows[2],
            vec![
                "welcome",
                "Hi <b><xliff:g id=\"name\">%1$s</xliff:g></b> & co",
                ""
            ]
        );
        assert_eq!(rows[3], vec!["files (one)", "%d file", ""]);
        assert_eq!(rows[6], vec!["days[1]", "Tue", ""]);
    }

    #[test]
    fn entities_next_to_multibyte_text() {
        assert_eq!(decode_entities("&amp; أهلاً بك"), "& أهلاً بك");
        assert_eq!(decode_entities("a &أهلاً بك; b"), "a &أهلاً بك; b");
        assert_eq!(decode_entities("&#1571;&lt;"), "أ<");
    }

    #[test]
    fn resx_entries() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<root>
  <resheader name="resmimetype"><value>text/microsoft-resx</value></resheader>
  <data name="Title" xml:space="preserve"><value>Hello</value><comment>Window title</comment></data>
  <data name="Logo" type="System.Drawing.Bitmap, System.Drawing" mimetype="application/x-microsoft.net.object.bytearray.base64"><value>AAAA</value></data>
</root>"#;
        let rows = resx(xml).unwrap().remove(0).rows;
        assert_eq!(rows[1], vec!["Title", "Hello", "Window title"]);
        assert_eq!(
            rows[2],
            vec!["Logo", "(System.Drawing.Bitmap, 4 bytes)", ""]
        );
    }
}
