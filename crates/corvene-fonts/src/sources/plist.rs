//! XML read into a small element tree (shared by `.glif` and
//! `.designspace`), and Apple XML property lists on top of it (UFO
//! `contents.plist`, a glyph's `<lib>`).

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::FontError;

/// Deeper documents are refused: the tree is walked and dropped
/// recursively.
const MAX_DEPTH: usize = 256;

/// One XML element with its attributes, child elements and the text
/// directly inside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// An attribute read as a number (`None` when missing or not a number).
    pub fn num(&self, name: &str) -> Option<f32> {
        self.attr(name).and_then(|v| v.trim().parse::<f32>().ok())
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }
}

/// Reads a whole XML document; returns its root element.
pub(crate) fn parse_xml(bytes: &[u8], format: &'static str) -> Result<Element, FontError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut reader = Reader::from_reader(bytes);
    let bad = |reader: &Reader<&[u8]>, e: &dyn std::fmt::Display| {
        FontError::malformed(format, format!("{e} at byte {}", reader.error_position()))
    };
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    loop {
        let event = match reader.read_event() {
            Ok(e) => e,
            Err(e) => return Err(bad(&reader, &e)),
        };
        match event {
            Event::Start(_) | Event::Empty(_) if root.is_some() && stack.is_empty() => {
                return Err(FontError::malformed(format, "more than one root element"));
            }
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(FontError::malformed(format, "elements nested too deeply"));
                }
                stack.push(start_element(&e, format)?);
            }
            Event::Empty(e) => {
                let el = start_element(&e, format)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::End(_) => {
                let Some(el) = stack.pop() else {
                    return Err(FontError::malformed(format, "unexpected end tag"));
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    let s = t.xml10_content().map_err(|e| bad(&reader, &e))?;
                    top.text.push_str(&s);
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    let s = t.decode().map_err(|e| bad(&reader, &e))?;
                    top.text.push_str(&s);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    if r.is_char_ref() {
                        if let Ok(Some(c)) = r.resolve_char_ref() {
                            top.text.push(c);
                        }
                    } else {
                        let name = r.decode().map_err(|e| bad(&reader, &e))?;
                        match quick_xml::escape::resolve_predefined_entity(&name) {
                            Some(s) => top.text.push_str(s),
                            None => {
                                top.text.push('&');
                                top.text.push_str(&name);
                                top.text.push(';');
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
    if !stack.is_empty() {
        return Err(FontError::malformed(format, "unclosed element"));
    }
    root.ok_or_else(|| FontError::malformed(format, "no root element"))
}

fn start_element(
    e: &quick_xml::events::BytesStart<'_>,
    format: &'static str,
) -> Result<Element, FontError> {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|err| FontError::malformed(format, err.to_string()))?;
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let value = a
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| FontError::malformed(format, err.to_string()))?;
        attrs.push((key, value.into_owned()));
    }
    Ok(Element {
        name,
        attrs,
        children: Vec::new(),
        text: String::new(),
    })
}

/// A property list value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Plist {
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    String(String),
    Integer(i64),
    Real(f64),
    Bool(bool),
    Data(String),
    Date(String),
}

impl Plist {
    pub fn keys(&self) -> Vec<String> {
        match self {
            Plist::Dict(entries) => entries.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        }
    }
}

/// A plist value element (`<dict>`, `<array>`, `<string>`…); `None` for
/// anything else.
pub(crate) fn plist_value(el: &Element) -> Option<Plist> {
    Some(match el.name.as_str() {
        "dict" => {
            let mut entries = Vec::new();
            let mut key: Option<&str> = None;
            for c in &el.children {
                if c.name == "key" {
                    key = Some(c.text.as_str());
                } else if let Some(k) = key.take()
                    && let Some(v) = plist_value(c)
                {
                    entries.push((k.to_string(), v));
                }
            }
            Plist::Dict(entries)
        }
        "array" => Plist::Array(el.children.iter().filter_map(plist_value).collect()),
        "string" => Plist::String(el.text.clone()),
        "integer" => {
            let t = el.text.trim();
            match t.parse::<i64>() {
                Ok(v) => Plist::Integer(v),
                Err(_) => Plist::Real(t.parse::<f64>().unwrap_or(0.0)),
            }
        }
        "real" => Plist::Real(el.text.trim().parse::<f64>().unwrap_or(0.0)),
        "true" => Plist::Bool(true),
        "false" => Plist::Bool(false),
        "data" => Plist::Data(el.text.split_whitespace().collect()),
        "date" => Plist::Date(el.text.trim().to_string()),
        _ => return None,
    })
}

/// Reads an XML property list document; returns its top value.
pub(crate) fn parse_plist(bytes: &[u8], format: &'static str) -> Result<Plist, FontError> {
    let root = parse_xml(bytes, format)?;
    let value_el = if root.name == "plist" {
        root.children
            .first()
            .ok_or_else(|| FontError::malformed(format, "empty property list"))?
    } else {
        &root
    };
    plist_value(value_el).ok_or_else(|| FontError::malformed(format, "not a property list"))
}

/// UFO `glyphs/contents.plist`: glyph name → `.glif` file name.
pub fn parse_contents_plist(bytes: &[u8]) -> Result<BTreeMap<String, String>, FontError> {
    const FORMAT: &str = "contents.plist";
    match parse_plist(bytes, FORMAT)? {
        Plist::Dict(entries) => Ok(entries
            .into_iter()
            .filter_map(|(k, v)| match v {
                Plist::String(file) => Some((k, file)),
                _ => None,
            })
            .collect()),
        _ => Err(FontError::malformed(FORMAT, "top level is not a dict")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_contents_plist() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>A</key>
    <string>A_.glif</string>
    <key>a&amp;b</key>
    <string>a&#38;b.glif</string>
  </dict>
</plist>"#;
        let map = parse_contents_plist(xml).unwrap();
        assert_eq!(map.get("A").map(String::as_str), Some("A_.glif"));
        assert_eq!(map.get("a&b").map(String::as_str), Some("a&b.glif"));
    }

    #[test]
    fn reads_nested_values() {
        let xml = b"<plist><dict><key>n</key><integer>3</integer><key>r</key><real>1.5</real>\
<key>b</key><true/><key>l</key><array><string>x</string><dict/></array></dict></plist>";
        let v = parse_plist(xml, "plist").unwrap();
        assert_eq!(
            v,
            Plist::Dict(vec![
                ("n".into(), Plist::Integer(3)),
                ("r".into(), Plist::Real(1.5)),
                ("b".into(), Plist::Bool(true)),
                (
                    "l".into(),
                    Plist::Array(vec![Plist::String("x".into()), Plist::Dict(vec![])])
                ),
            ])
        );
        assert_eq!(v.keys(), vec!["n", "r", "b", "l"]);
    }

    #[test]
    fn refuses_broken_xml() {
        assert!(parse_xml(b"<a><b></a>", "x").is_err());
        assert!(parse_xml(b"<a>", "x").is_err());
        assert!(parse_xml(b"", "x").is_err());
        assert!(parse_contents_plist(b"<plist><array/></plist>").is_err());
        let deep = "<a>".repeat(MAX_DEPTH + 1);
        assert!(parse_xml(deep.as_bytes(), "x").is_err());
    }
}
