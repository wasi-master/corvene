//! Corvene `1355-email-diff`: mail as it reads, not as it is encoded. The
//! headers that matter (sender, recipients, date, subject, thread ids)
//! decoded, the text body decoded from base64 or quoted-printable and its
//! charset (an HTML-only body turned into text), and the attachments
//! listed with their type, size and a short hash.
//!
//! Formats: `.eml` (RFC 5322 / MIME), `.emlx` (Apple Mail), `.mbox`
//! (one section per message) and Outlook `.msg` files ([`msg`]).

pub mod html;
pub mod msg;

use std::fmt::Write as _;

use mail_parser::{Address, MessageParser, MimeHeaders as _};

use crate::Format;

/// Messages of a mailbox printed; the rest are counted.
const MAX_MESSAGES: usize = 5_000;

/// The text of one side.
pub fn to_text(format: Format, bytes: &[u8]) -> Result<String, String> {
    match format {
        Format::Eml => message_text(bytes),
        Format::Emlx => message_text(emlx_message(bytes)?),
        Format::Mbox => mbox_text(bytes),
        Format::Msg => msg::to_text(bytes),
        _ => Err("Not a mail file.".into()),
    }
}

/// The RFC 5322 message inside an `.emlx` file (a byte count line, the
/// message, then a property list of flags).
fn emlx_message(bytes: &[u8]) -> Result<&[u8], String> {
    let nl = bytes
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("Not an Apple Mail message.")?;
    let count: usize = std::str::from_utf8(&bytes[..nl])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .ok_or("Not an Apple Mail message.")?;
    let start = nl + 1;
    bytes
        .get(start..start + count)
        .or_else(|| bytes.get(start..))
        .ok_or_else(|| "Not an Apple Mail message.".to_string())
}

fn addresses(address: Option<&Address<'_>>) -> Option<String> {
    let address = address?;
    let list: Vec<String> = address
        .iter()
        .map(|a| match (&a.name, &a.address) {
            (Some(name), Some(addr)) if !name.is_empty() => format!("{name} <{addr}>"),
            (_, Some(addr)) => addr.to_string(),
            (Some(name), None) => name.to_string(),
            (None, None) => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect();
    (!list.is_empty()).then(|| list.join(", "))
}

/// One RFC 5322 message as text.
pub(crate) fn message_text(bytes: &[u8]) -> Result<String, String> {
    let message = MessageParser::default()
        .parse(bytes)
        .ok_or("Not a readable email message.")?;
    if message.headers().is_empty() {
        return Err("Not an email message (no headers).".into());
    }
    let mut out = String::new();
    let mut header = |name: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            let _ = writeln!(out, "{name}: {}", value.replace('\n', " "));
        }
    };
    header("From", addresses(message.from()));
    header("Sender", addresses(message.sender()));
    header("Reply-To", addresses(message.reply_to()));
    header("To", addresses(message.to()));
    header("Cc", addresses(message.cc()));
    header("Bcc", addresses(message.bcc()));
    header("Date", message.date().map(|d| d.to_rfc3339()));
    header("Subject", message.subject().map(str::to_string));
    header(
        "Message-ID",
        message.message_id().map(|id| format!("<{id}>")),
    );
    let ids = |v: &mail_parser::HeaderValue<'_>| -> Option<String> {
        v.as_text_list()
            .map(|l| {
                l.iter()
                    .map(|id| format!("<{id}>"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .or_else(|| v.as_text().map(|id| format!("<{id}>")))
    };
    header("In-Reply-To", ids(message.in_reply_to()));
    header("References", ids(message.references()));
    for name in ["List-Id", "X-Mailer", "User-Agent"] {
        header(
            name,
            message
                .header(name)
                .and_then(|h| h.as_text())
                .map(str::to_string),
        );
    }
    out.push('\n');
    let mut bodies = 0;
    for ix in 0..message.text_body_count() {
        let Some(part) = message.text_part(ix as u32) else {
            continue;
        };
        let is_html = part.is_content_type("text", "html");
        let body = if is_html {
            part.text_contents().map(html::to_text)
        } else {
            message.body_text(ix).map(|t| t.into_owned())
        };
        let Some(body) = body else {
            continue;
        };
        if bodies > 0 {
            out.push_str("\n----\n\n");
        }
        if is_html {
            out.push_str("(HTML body, as text)\n\n");
        }
        out.push_str(body.replace("\r\n", "\n").trim_end());
        out.push('\n');
        bodies += 1;
    }
    let attachments: Vec<String> = message
        .attachments()
        .map(|part| {
            let name = part
                .attachment_name()
                .map(str::to_string)
                .or_else(|| {
                    part.message()
                        .and_then(|m| m.subject())
                        .map(|s| format!("{s}.eml"))
                })
                .unwrap_or_else(|| "(unnamed)".into());
            let ty = part
                .content_type()
                .map(|t| match &t.c_subtype {
                    Some(sub) => format!("{}/{sub}", t.c_type),
                    None => t.c_type.to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into());
            let contents = part.contents();
            format!(
                "  {name}  {ty}  {} bytes  {:08x}",
                crate::out::thousands(contents.len() as u64),
                fnv(contents)
            )
        })
        .collect();
    if !attachments.is_empty() {
        out.push_str("\nAttachments\n");
        for line in attachments {
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// FNV-1a, a short fingerprint of an attachment's bytes.
pub(crate) fn fnv(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in bytes {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A mailbox: every message, split at its `From ` line.
fn mbox_text(bytes: &[u8]) -> Result<String, String> {
    let mut starts = Vec::new();
    if bytes.starts_with(b"From ") {
        starts.push(0);
    }
    for (ix, window) in bytes.windows(6).enumerate() {
        if window == b"\nFrom " {
            starts.push(ix + 1);
        }
    }
    if starts.is_empty() {
        return Err("Not a mailbox (no From lines).".into());
    }
    let mut out = String::new();
    let total = starts.len();
    for (n, start) in starts.iter().enumerate().take(MAX_MESSAGES) {
        let end = starts.get(n + 1).copied().unwrap_or(bytes.len());
        let chunk = &bytes[*start..end];
        // the envelope line is not part of the message
        let body = chunk
            .iter()
            .position(|b| *b == b'\n')
            .map_or(&chunk[chunk.len()..], |nl| &chunk[nl + 1..]);
        // `>From ` lines were escaped when the mailbox was written
        let unescaped = unescape_from(body);
        let _ = writeln!(out, "=== Message {} of {total} ===", n + 1);
        match message_text(&unescaped) {
            Ok(text) => out.push_str(&text),
            Err(e) => {
                let _ = writeln!(out, "({e})");
            }
        }
        out.push('\n');
    }
    if total > MAX_MESSAGES {
        let _ = writeln!(out, "… {} more messages", total - MAX_MESSAGES);
    }
    Ok(out)
}

fn unescape_from(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut line_start = true;
    let mut i = 0;
    while i < body.len() {
        if line_start && body[i] == b'>' {
            let rest = &body[i..];
            let quoted = rest.iter().take_while(|b| **b == b'>').count();
            if rest[quoted..].starts_with(b"From ") {
                i += 1;
                line_start = false;
                continue;
            }
        }
        line_start = body[i] == b'\n';
        out.push(body[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIL: &str = "From: Alice Example <alice@example.com>\r\n\
To: bob@example.com\r\n\
Subject: =?UTF-8?B?SMOpbGxv?=\r\n\
Date: Tue, 2 Jan 2024 10:00:00 +0100\r\n\
Message-ID: <abc@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Caf=C3=A9 at noon?\r\n\
--b\r\n\
Content-Type: application/pdf; name=\"plan.pdf\"\r\n\
Content-Disposition: attachment; filename=\"plan.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0=\r\n\
--b--\r\n";

    #[test]
    fn decodes_headers_body_and_attachments() {
        let text = to_text(Format::Eml, MAIL.as_bytes()).unwrap();
        assert!(
            text.contains("From: Alice Example <alice@example.com>"),
            "{text}"
        );
        assert!(text.contains("Subject: Héllo"), "{text}");
        assert!(text.contains("Date: 2024-01-02T10:00:00+01:00"), "{text}");
        assert!(text.contains("Café at noon?"), "{text}");
        assert!(
            text.contains("plan.pdf  application/pdf  5 bytes"),
            "{text}"
        );
    }

    #[test]
    fn mailboxes_split_by_from_lines() {
        let mbox = format!(
            "From alice@example.com Tue Jan  2 10:00:00 2024\n{}\nFrom bob@example.com Tue Jan  2 11:00:00 2024\nFrom: bob@example.com\nSubject: Two\n\n>From the start\n",
            MAIL.replace("\r\n", "\n")
        );
        let text = to_text(Format::Mbox, mbox.as_bytes()).unwrap();
        assert!(text.contains("=== Message 1 of 2 ==="), "{text}");
        assert!(
            text.contains("=== Message 2 of 2 ===\nFrom: bob@example.com"),
            "{text}"
        );
        assert!(text.contains("\nFrom the start"), "{text}");
    }

    #[test]
    fn emlx_strips_its_wrapper() {
        let mail = "From: a@b.c\nSubject: Hi\n\nBody\n";
        let emlx = format!("{}\n{mail}<?xml version=\"1.0\"?><plist/>", mail.len());
        let text = to_text(Format::Emlx, emlx.as_bytes()).unwrap();
        assert!(text.contains("Subject: Hi"), "{text}");
        assert!(text.contains("Body"), "{text}");
        assert!(!text.contains("plist"), "{text}");
    }
}
