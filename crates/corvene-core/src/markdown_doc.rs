//! Corvene `1334-markdown-preview-diff`: a Markdown file read into blocks
//! for its rendered diff. YAML (`---`) or TOML (`+++`) front matter becomes
//! a code block, MDX `import` / `export` lines are left out, and each list
//! item is a block of its own, so an edit to one item marks that item only
//! (`crate::block_diff` aligns whole blocks).
//!
//! GitHub Desktop shows Markdown as its text diff only; github.com's rich
//! diff renders the front matter as a table and the rest as here.

use crate::markdown::{Block, ParseOptions, parse_with};

/// The blocks of a Markdown file (`mdx`: an MDX file; `math`: typeset
/// `$…$` as GitHub does).
pub fn parse_document(text: &str, mdx: bool, math: bool) -> Vec<Block> {
    let (front, body) = front_matter(text);
    let body = if mdx {
        strip_mdx(body)
    } else {
        body.to_string()
    };
    let mut out = Vec::new();
    if let Some((language, code)) = front {
        out.push(Block::CodeBlock {
            language: Some(language.to_string()),
            code: code.to_string(),
        });
    }
    for block in parse_with(&body, ParseOptions { math }) {
        match block {
            Block::List { start, items } if items.len() > 1 => {
                for (i, item) in items.into_iter().enumerate() {
                    out.push(Block::List {
                        start: start.map(|s| s + i as u64),
                        items: vec![item],
                    });
                }
            }
            block => out.push(block),
        }
    }
    out
}

/// Whether `path` is an MDX file.
pub fn is_mdx(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case("mdx"))
}

/// The front matter's language and text, and the rest of the document.
fn front_matter(text: &str) -> (Option<(&'static str, &str)>, &str) {
    for (fence, language) in [("---", "yaml"), ("+++", "toml")] {
        let Some(rest) = text
            .strip_prefix(fence)
            .and_then(|r| r.strip_prefix("\r\n").or_else(|| r.strip_prefix('\n')))
        else {
            continue;
        };
        let mut offset = 0;
        for line in rest.split_inclusive('\n') {
            let bare = line.trim_end_matches(['\r', '\n']);
            if bare == fence || (language == "yaml" && bare == "...") {
                let code = rest[..offset].trim_end_matches(['\r', '\n']);
                return (Some((language, code)), &rest[offset + line.len()..]);
            }
            offset += line.len();
        }
    }
    (None, text)
}

/// MDX without its top-level `import` / `export` statements (outside code
/// fences); JSX tags stay and render as HTML would.
fn strip_mdx(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    let mut in_statement = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        }
        if !fenced && (line.starts_with("import ") || line.starts_with("export ")) {
            // a multi-line statement runs until a line ending its braces
            in_statement = line.contains('{') && !line.contains('}');
            continue;
        }
        if in_statement {
            if line.contains('}') {
                in_statement = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_becomes_code() {
        let blocks = parse_document("---\ntitle: Hi\n---\n# Head\n", false, false);
        assert!(matches!(
            &blocks[0],
            Block::CodeBlock { language: Some(l), code } if l == "yaml" && code == "title: Hi"
        ));
        assert!(matches!(&blocks[1], Block::Heading { level: 1, .. }));
        // a rule that is not front matter stays a rule
        assert!(matches!(
            parse_document("text\n\n---\n\nmore", false, false)[1],
            Block::Rule
        ));
    }

    #[test]
    fn list_items_are_blocks() {
        let blocks = parse_document("3. a\n4. b\n\n- x\n- y\n", false, false);
        let starts: Vec<Option<u64>> = blocks
            .iter()
            .map(|b| match b {
                Block::List { start, items } => {
                    assert_eq!(items.len(), 1);
                    *start
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(starts, [Some(3), Some(4), None, None]);
    }

    #[test]
    fn mdx_statements_are_left_out() {
        let text = "import A from './a'\nexport const meta = {\n  title: 'x',\n}\n\n# Title\n\n```js\nimport kept\n```\n";
        let blocks = parse_document(text, true, false);
        assert!(matches!(&blocks[0], Block::Heading { .. }), "{blocks:?}");
        assert!(
            matches!(&blocks[1], Block::CodeBlock { code, .. } if code.contains("import kept"))
        );
        assert!(is_mdx("a/b.MDX"));
    }
}
