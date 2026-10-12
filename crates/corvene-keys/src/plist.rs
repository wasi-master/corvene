//! A small XML property list reader.

/// Deepest nesting read.
const MAX_DEPTH: usize = 32;

/// One plist value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    String(String),
    /// Integers and reals kept as written.
    Number(String),
    Bool(bool),
    Date(String),
    Data(Vec<u8>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::Number(s) | Value::Date(s) => Some(s),
            _ => None,
        }
    }

    pub fn bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Short display form (arrays joined, nested dicts summarised).
    pub fn display(&self) -> String {
        match self {
            Value::String(s) | Value::Number(s) | Value::Date(s) => s.clone(),
            Value::Bool(b) => if *b { "true" } else { "false" }.into(),
            Value::Data(d) => format!("{} bytes", d.len()),
            Value::Array(a) => a.iter().map(Value::display).collect::<Vec<_>>().join(", "),
            Value::Dict(d) => format!("{} keys", d.len()),
        }
    }
}

/// Parses XML plist text.
pub fn parse(text: &str) -> Option<Value> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 1_000_000,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, opts).ok()?;
    let root = doc.root_element();
    let node = if root.has_tag_name("plist") {
        root.children().find(|n| n.is_element())?
    } else {
        root
    };
    value(node, 0)
}

fn value(n: roxmltree::Node, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }
    let text = || n.text().unwrap_or("").to_string();
    Some(match n.tag_name().name() {
        "dict" => {
            let mut out = Vec::new();
            let mut key = None;
            for c in n.children().filter(|c| c.is_element()) {
                if c.has_tag_name("key") {
                    key = Some(c.text().unwrap_or("").to_string());
                } else if let Some(k) = key.take()
                    && let Some(v) = value(c, depth + 1)
                {
                    out.push((k, v));
                }
            }
            Value::Dict(out)
        }
        "array" => Value::Array(
            n.children()
                .filter(|c| c.is_element())
                .filter_map(|c| value(c, depth + 1))
                .collect(),
        ),
        "string" => Value::String(text()),
        "integer" | "real" => Value::Number(text().trim().to_string()),
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "date" => Value::Date(text().trim().to_string()),
        "data" => Value::Data(crate::util::b64(&text()).unwrap_or_default()),
        _ => return None,
    })
}
