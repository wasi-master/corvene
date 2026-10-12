//! Outlook `.msg` files (MS-OXMSG): a compound file whose streams hold the
//! message's MAPI properties. Read: sender, recipients, the submit time,
//! subject, the plain text body (or the HTML one as text), the transport
//! headers' Message-ID, and the attachments with their names and sizes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read as _;

/// Stream bytes read in all (a hostile file could claim huge streams).
const MAX_STREAM_BYTES: u64 = 32 << 20;

/// Every stream of the compound file, by path (`/__substg1.0_0037001F`).
fn streams(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut file = cfb::CompoundFile::open(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Not an Outlook message: {e}."))?;
    let paths: Vec<(String, u64)> = file
        .walk()
        .filter(|e| e.is_stream())
        .map(|e| (e.path().to_string_lossy().replace('\\', "/"), e.len()))
        .collect();
    let mut out = BTreeMap::new();
    let mut budget = MAX_STREAM_BYTES;
    for (path, len) in paths {
        // attachment contents are only measured
        if path.ends_with("__substg1.0_37010102") {
            out.insert(path, len.to_le_bytes().to_vec());
            continue;
        }
        if len > budget {
            continue;
        }
        budget -= len;
        let mut data = Vec::with_capacity(len as usize);
        if let Ok(mut stream) = file.open_stream(&path) {
            let _ = stream.read_to_end(&mut data);
        }
        out.insert(path, data);
    }
    Ok(out)
}

/// A string property under `prefix` (Unicode `001F` or 8-bit `001E`).
fn string(streams: &BTreeMap<String, Vec<u8>>, prefix: &str, tag: &str) -> Option<String> {
    if let Some(data) = streams.get(&format!("{prefix}/__substg1.0_{tag}001F")) {
        let units: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let s = String::from_utf16_lossy(&units);
        return Some(s.trim_end_matches('\0').to_string());
    }
    let data = streams.get(&format!("{prefix}/__substg1.0_{tag}001E"))?;
    let (text, _, _) = encoding_rs::WINDOWS_1252.decode(data);
    Some(text.trim_end_matches('\0').to_string())
}

fn binary<'a>(streams: &'a BTreeMap<String, Vec<u8>>, prefix: &str, tag: &str) -> Option<&'a [u8]> {
    streams
        .get(&format!("{prefix}/__substg1.0_{tag}0102"))
        .map(Vec::as_slice)
}

/// A fixed-size property from a `__properties_version1.0` stream (the top
/// level one has a 32-byte header, others 8): `tag` is `id << 16 | type`.
fn fixed(
    streams: &BTreeMap<String, Vec<u8>>,
    prefix: &str,
    header: usize,
    tag: u32,
) -> Option<u64> {
    let data = streams.get(&format!("{prefix}/__properties_version1.0"))?;
    data.get(header..)?
        .as_chunks::<16>()
        .0
        .iter()
        .find_map(|entry| {
            let t = u32::from_le_bytes(entry[0..4].try_into().ok()?);
            (t == tag).then(|| u64::from_le_bytes(entry[8..16].try_into().unwrap_or_default()))
        })
}

/// The text of one `.msg` file.
pub fn to_text(bytes: &[u8]) -> Result<String, String> {
    let streams = streams(bytes)?;
    if !streams.keys().any(|k| k.starts_with("/__substg1.0_")) {
        return Err("Not an Outlook message (no properties).".into());
    }
    let mut out = String::new();
    let root = "";
    let sender = match (
        string(&streams, root, "0C1A"),
        string(&streams, root, "5D01").or_else(|| string(&streams, root, "0C1F")),
    ) {
        (Some(name), Some(addr)) if !name.is_empty() && name != addr => {
            Some(format!("{name} <{addr}>"))
        }
        (_, Some(addr)) => Some(addr),
        (name, None) => name,
    };
    let mut header = |name: &str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            let _ = writeln!(out, "{name}: {v}");
        }
    };
    header("From", sender);
    // recipients by type (1 to, 2 cc, 3 bcc), else the display strings
    let mut kinds: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    let recipients: Vec<String> = streams
        .keys()
        .filter_map(|k| {
            let rest = k.strip_prefix("/__recip_version1.0_#")?;
            Some(format!("/__recip_version1.0_#{}", rest.split('/').next()?))
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for prefix in &recipients {
        let name = string(&streams, prefix, "3001").unwrap_or_default();
        let addr = string(&streams, prefix, "39FE")
            .or_else(|| string(&streams, prefix, "3003"))
            .unwrap_or_default();
        let shown = match (name.is_empty(), addr.is_empty()) {
            (false, false) if name != addr => format!("{name} <{addr}>"),
            (_, false) => addr,
            _ => name,
        };
        let kind = fixed(&streams, prefix, 8, 0x0C15_0003).unwrap_or(1) & 0xffff_ffff;
        kinds.entry(kind).or_default().push(shown);
    }
    let shown = |k: u64, fallback: &str| {
        kinds
            .get(&k)
            .map(|l| l.join(", "))
            .or_else(|| string(&streams, root, fallback))
    };
    header("To", shown(1, "0E04"));
    header("Cc", shown(2, "0E03"));
    header("Bcc", shown(3, "0E02"));
    let sent = fixed(&streams, root, 32, 0x0039_0040)
        .or_else(|| fixed(&streams, root, 32, 0x0E06_0040))
        .filter(|t| *t > 0)
        .map(filetime);
    header("Date", sent);
    header("Subject", string(&streams, root, "0037"));
    let transport = string(&streams, root, "007D").unwrap_or_default();
    let message_id = string(&streams, root, "1035").or_else(|| {
        transport
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("message-id:"))
            .map(|l| l[11..].trim().to_string())
    });
    header("Message-ID", message_id);
    out.push('\n');
    let body = string(&streams, root, "1000").filter(|b| !b.trim().is_empty());
    match body {
        Some(body) => out.push_str(body.replace("\r\n", "\n").trim_end()),
        None => {
            if let Some(html) = binary(&streams, root, "1013") {
                out.push_str("(HTML body, as text)\n\n");
                out.push_str(&super::html::to_text(&String::from_utf8_lossy(html)));
            }
        }
    }
    out.push('\n');
    let attachments: Vec<String> = streams
        .keys()
        .filter_map(|k| {
            let rest = k.strip_prefix("/__attach_version1.0_#")?;
            Some(format!("/__attach_version1.0_#{}", rest.split('/').next()?))
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|prefix| {
            let name = string(&streams, &prefix, "3707")
                .or_else(|| string(&streams, &prefix, "3704"))
                .or_else(|| string(&streams, &prefix, "3001"))
                .unwrap_or_else(|| "(unnamed)".into());
            let ty = string(&streams, &prefix, "370E").unwrap_or_default();
            let size = streams
                .get(&format!("{prefix}/__substg1.0_37010102"))
                .and_then(|b| b.as_slice().try_into().ok())
                .map(u64::from_le_bytes);
            let mut line = format!("  {name}");
            if !ty.is_empty() {
                let _ = write!(line, "  {ty}");
            }
            if let Some(size) = size {
                let _ = write!(line, "  {} bytes", crate::out::thousands(size));
            }
            line
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

/// A Windows FILETIME (100 ns ticks since 1601) as UTC.
fn filetime(ticks: u64) -> String {
    let secs = (ticks / 10_000_000) as i64 - 11_644_473_600;
    crate::compiled::pyc::utc(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn reads_an_outlook_message() {
        let mut file = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
        fn put(file: &mut cfb::CompoundFile<std::io::Cursor<Vec<u8>>>, path: &str, data: &[u8]) {
            file.create_stream(path).unwrap().write_all(data).unwrap();
        }
        put(&mut file, "/__substg1.0_0037001F", &utf16("Quarterly plan"));
        put(&mut file, "/__substg1.0_0C1A001F", &utf16("Alice"));
        put(
            &mut file,
            "/__substg1.0_0C1F001F",
            &utf16("alice@example.com"),
        );
        put(&mut file, "/__substg1.0_0E04001F", &utf16("Bob"));
        put(
            &mut file,
            "/__substg1.0_1000001F",
            &utf16("See attached.\r\nThanks"),
        );
        let mut props = vec![0u8; 32];
        props.extend(0x0039_0040u32.to_le_bytes());
        props.extend(0u32.to_le_bytes());
        // 2024-01-02 09:00:00 UTC as FILETIME
        props.extend(((1_704_186_000u64 + 11_644_473_600) * 10_000_000).to_le_bytes());
        put(&mut file, "/__properties_version1.0", &props);
        file.create_storage("/__attach_version1.0_#00000000")
            .unwrap();
        put(
            &mut file,
            "/__attach_version1.0_#00000000/__substg1.0_3707001F",
            &utf16("plan.xlsx"),
        );
        put(
            &mut file,
            "/__attach_version1.0_#00000000/__substg1.0_37010102",
            &[0u8; 1234],
        );
        file.flush().unwrap();
        let bytes = file.into_inner().into_inner();
        let text = to_text(&bytes).unwrap();
        assert!(text.contains("From: Alice <alice@example.com>"), "{text}");
        assert!(text.contains("To: Bob"), "{text}");
        assert!(text.contains("Date: 2024-01-02 09:00:00 UTC"), "{text}");
        assert!(text.contains("Subject: Quarterly plan"), "{text}");
        assert!(text.contains("See attached.\nThanks"), "{text}");
        assert!(text.contains("  plan.xlsx  1,234 bytes"), "{text}");
    }
}
