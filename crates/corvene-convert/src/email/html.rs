//! HTML (a mail body) as plain text: block elements start lines, list
//! items get a bullet, links keep their address, scripts, styles and the
//! head are dropped and character references are decoded.

/// The text of an HTML document or fragment.
pub fn to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut chars = html.char_indices().peekable();
    let mut skip_until: Option<&'static str> = None;
    let mut pending_link: Vec<Option<String>> = Vec::new();
    let mut line = String::new();
    let flush = |line: &mut String, out: &mut String| {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            out.push_str(&collapsed);
            out.push('\n');
        } else if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        line.clear();
    };
    while let Some((ix, ch)) = chars.next() {
        if ch != '<' {
            if skip_until.is_none() {
                if ch == '&' {
                    let rest = &html[ix..];
                    if let Some((decoded, len)) = entity(rest) {
                        line.push_str(&decoded);
                        for _ in 1..len {
                            chars.next();
                        }
                        continue;
                    }
                }
                line.push(ch);
            }
            continue;
        }
        // a tag (or comment) up to its `>`
        let rest = &html[ix..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(rest.len(), |e| e + 3);
            advance(&mut chars, ix + end);
            continue;
        }
        let end = rest.find('>').map_or(rest.len(), |e| e + 1);
        let tag = &rest[1..end.saturating_sub(1).max(1)];
        advance(&mut chars, ix + end);
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if let Some(until) = skip_until {
            if closing && name == until {
                skip_until = None;
            }
            continue;
        }
        match name.as_str() {
            "script" | "style" | "head" | "title" if !closing => {
                skip_until = Some(match name.as_str() {
                    "script" => "script",
                    "style" => "style",
                    "head" => "head",
                    _ => "title",
                });
            }
            "br" => flush(&mut line, &mut out),
            "p" | "div" | "tr" | "table" | "blockquote" | "pre" | "section" | "article"
            | "header" | "footer" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "hr" => flush(&mut line, &mut out),
            "li" if !closing => {
                flush(&mut line, &mut out);
                line.push_str("- ");
            }
            "td" | "th" if !closing && !line.trim().is_empty() => line.push_str(" | "),
            "img" => {
                if let Some(alt) = attribute(tag, "alt").filter(|a| !a.is_empty()) {
                    line.push_str(&format!("[{alt}]"));
                }
            }
            "a" if !closing => pending_link.push(attribute(tag, "href")),
            "a" => {
                if let Some(Some(href)) = pending_link.pop()
                    && !href.starts_with('#')
                    && !line.contains(&href)
                {
                    line.push_str(&format!(" <{href}>"));
                }
            }
            _ => {}
        }
    }
    flush(&mut line, &mut out);
    out.trim().to_string()
}

fn advance(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>, to: usize) {
    while let Some(&(ix, _)) = chars.peek() {
        if ix >= to {
            break;
        }
        chars.next();
    }
}

/// The value of attribute `name` in a tag's text.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let value_start = tag.len() - rest.len() + 1;
        let value = tag[value_start..].trim_start();
        let (value, _) = match value.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let inner = &value[1..];
                (
                    inner[..inner.find(q).unwrap_or(inner.len())].to_string(),
                    (),
                )
            }
            _ => (
                value
                    .split(|c: char| c.is_whitespace() || c == '>')
                    .next()
                    .unwrap_or("")
                    .to_string(),
                (),
            ),
        };
        let mut decoded = String::new();
        let mut rest = value.as_str();
        while let Some(amp) = rest.find('&') {
            decoded.push_str(&rest[..amp]);
            match entity(&rest[amp..]) {
                Some((text, len)) => {
                    decoded.push_str(&text);
                    rest = &rest[amp + len..];
                }
                None => {
                    decoded.push('&');
                    rest = &rest[amp + 1..];
                }
            }
        }
        decoded.push_str(rest);
        return Some(decoded);
    }
    None
}

/// A character reference at the start of `s`: its text and byte length.
fn entity(s: &str) -> Option<(String, usize)> {
    // Bytes, not a str slice: byte 12 can fall inside a multi-byte char.
    let semi = s.bytes().take(12).position(|b| b == b';')?;
    let body = &s[1..semi];
    let text = if let Some(num) = body.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        char::from_u32(code)?.to_string()
    } else {
        match body {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            "nbsp" => " ",
            "copy" => "©",
            "reg" => "®",
            "trade" => "™",
            "hellip" => "…",
            "mdash" => "—",
            "ndash" => "–",
            "lsquo" => "‘",
            "rsquo" => "’",
            "ldquo" => "“",
            "rdquo" => "”",
            "bull" => "•",
            "middot" => "·",
            "euro" => "€",
            "pound" => "£",
            "yen" => "¥",
            "laquo" => "«",
            "raquo" => "»",
            "times" => "×",
            "deg" => "°",
            "zwnj" | "zwj" | "shy" => "",
            _ => return None,
        }
        .to_string()
    };
    Some((text, semi + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_reads_as_text() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body>\
            <p>Hello&nbsp;<b>world</b> &amp; friends</p><ul><li>one</li><li>two</li></ul>\
            <p>See <a href=\"https://e.com/a?b=1&amp;c=2\">the docs</a>.<br>Bye &#x1F44B;</p>\
            <!-- hidden --><script>alert(1)</script></body></html>";
        assert_eq!(
            to_text(html),
            "Hello world & friends\n\n- one\n- two\n\nSee the docs <https://e.com/a?b=1&c=2>.\nBye 👋"
        );
    }

    #[test]
    fn ampersand_before_multibyte_text() {
        assert_eq!(to_text("<p>a &أهلاًبك;</p>"), "a &أهلاًبك;");
    }
}
