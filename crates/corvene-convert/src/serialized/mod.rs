//! Corvene `1351-serialized-data-diff`: binary data formats as text.
//!
//! - binary property lists become XML property lists (the form Xcode and
//!   `plutil -convert xml1` write);
//! - MessagePack, CBOR, BSON and bencode (torrents) print as indented
//!   JSON-like text, with their extra types spelled out (`ObjectId(…)`,
//!   `tag 1(…)`, byte strings as hex);
//! - protocol buffer messages, which carry no schema, print as
//!   `protoc --decode_raw` does: field numbers with their values, nested
//!   messages where the bytes parse as one.

mod decode;

use std::fmt::Write as _;

use crate::Format;
pub(crate) use decode::Node;

/// Containers nested deeper are cut (a hostile file could recurse).
pub(crate) const MAX_DEPTH: usize = 128;

/// The text of one side.
pub fn to_text(format: Format, bytes: &[u8]) -> Result<String, String> {
    match format {
        Format::BinaryPlist => plist_xml(bytes),
        Format::MessagePack => decode::msgpack_all(bytes).map(|nodes| print_all(&nodes)),
        Format::Cbor => decode::cbor_all(bytes).map(|nodes| print_all(&nodes)),
        Format::Bson => decode::bson_all(bytes).map(|nodes| print_all(&nodes)),
        Format::Bencode => decode::bencode(bytes).map(|node| print_all(&[node])),
        Format::Protobuf => decode::protobuf_text(bytes),
        _ => Err("Not serialized data.".into()),
    }
}

/// A binary (or any) property list as an XML property list.
pub(crate) fn plist_xml(bytes: &[u8]) -> Result<String, String> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Not a readable property list: {e}."))?;
    let mut out = Vec::new();
    value
        .to_writer_xml(&mut out)
        .map_err(|e| format!("The property list could not be written: {e}."))?;
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn print_all(nodes: &[Node]) -> String {
    let mut out = String::new();
    for (ix, node) in nodes.iter().enumerate() {
        if nodes.len() > 1 {
            let _ = writeln!(out, "// document {}", ix + 1);
        }
        print(&mut out, node, 0);
        out.push('\n');
    }
    out
}

/// `node` as indented JSON-like text.
pub(crate) fn print(out: &mut String, node: &Node, depth: usize) {
    let pad = |out: &mut String, d: usize| {
        for _ in 0..d {
            out.push_str("  ");
        }
    };
    match node {
        Node::Null => out.push_str("null"),
        Node::Undefined => out.push_str("undefined"),
        Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Node::Int(v) => out.push_str(v),
        Node::Float(f) => {
            if f.is_finite() {
                let _ = write!(out, "{f:?}");
            } else {
                let _ = write!(out, "{f}");
            }
        }
        Node::Str(s) => out.push_str(&json_string(s)),
        Node::Bytes(b) => {
            let _ = write!(out, "<{}>", crate::out::hex(b));
        }
        Node::Typed(name, inner) => {
            let _ = write!(out, "{name}(");
            print(out, inner, depth);
            out.push(')');
        }
        Node::Array(items) if items.is_empty() => out.push_str("[]"),
        Node::Array(items) => {
            out.push_str("[\n");
            for (ix, item) in items.iter().enumerate() {
                pad(out, depth + 1);
                print(out, item, depth + 1);
                if ix + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(out, depth);
            out.push(']');
        }
        Node::Map(pairs) if pairs.is_empty() => out.push_str("{}"),
        Node::Map(pairs) => {
            out.push_str("{\n");
            for (ix, (key, value)) in pairs.iter().enumerate() {
                pad(out, depth + 1);
                match key {
                    Node::Str(s) => out.push_str(&json_string(s)),
                    other => print(out, other, depth + 1),
                }
                out.push_str(": ");
                print(out, value, depth + 1);
                if ix + 1 < pairs.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(out, depth);
            out.push('}');
        }
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_plists_become_xml() {
        let mut dict = plist::Dictionary::new();
        dict.insert(
            "CFBundleName".into(),
            plist::Value::String("Corvene".into()),
        );
        dict.insert("Version".into(), plist::Value::Integer(3.into()));
        let mut bin = Vec::new();
        plist::Value::Dictionary(dict)
            .to_writer_binary(&mut bin)
            .unwrap();
        assert!(bin.starts_with(b"bplist00"));
        let text = to_text(Format::BinaryPlist, &bin).unwrap();
        assert!(text.contains("<key>CFBundleName</key>"), "{text}");
        assert!(text.contains("<string>Corvene</string>"), "{text}");
        assert!(text.contains("<integer>3</integer>"), "{text}");
    }

    #[test]
    fn maps_print_indented() {
        let node = Node::Map(vec![
            (Node::Str("a".into()), Node::Int("1".into())),
            (
                Node::Str("b".into()),
                Node::Array(vec![Node::Bool(true), Node::Null]),
            ),
        ]);
        let text = print_all(&[node]);
        assert_eq!(
            text,
            "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    null\n  ]\n}\n"
        );
    }
}
