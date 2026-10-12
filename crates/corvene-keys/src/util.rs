//! Hex, base64, hashing and date helpers.

use base64::Engine;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// Decodes base64, ignoring whitespace and padding.
pub fn b64(text: &str) -> Option<Vec<u8>> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    LENIENT.decode(clean.as_bytes()).ok()
}

/// Base64 without padding (OpenSSH fingerprints).
pub fn b64_nopad(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";
const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(char::from(HEX_LOWER[usize::from(b >> 4)]));
        s.push(char::from(HEX_LOWER[usize::from(b & 15)]));
    }
    s
}

/// Colon separated hex (`AB:CD:...`), upper or lower case.
pub fn colon_hex(bytes: &[u8], upper: bool) -> String {
    let digits = if upper { HEX_UPPER } else { HEX_LOWER };
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        s.push(char::from(digits[usize::from(b >> 4)]));
        s.push(char::from(digits[usize::from(b & 15)]));
    }
    s
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    sha1_smol::Sha1::from(data).digest().bytes()
}

pub fn md5(data: &[u8]) -> [u8; 16] {
    use md5::Digest as _;
    md5::Md5::digest(data).into()
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn unix_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400
}

/// `2026-10-12 14:03:00 UTC`.
pub fn utc(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Parses an ISO 8601 UTC date (`2026-10-12T14:03:00Z`) as used in plists.
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let n = |a: usize, l: usize| -> Option<i64> {
        let p = s.get(a..a + l)?;
        p.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| p.parse().ok())
            .flatten()
    };
    let (y, mo, d, h, mi, se) = (
        n(0, 4)?,
        n(5, 2)?,
        n(8, 2)?,
        n(11, 2)?,
        n(14, 2)?,
        n(17, 2)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    Some(unix_from_civil(y, mo, d) + h * 3600 + mi * 60 + se)
}

/// Human length of a duration in seconds ("1 year 30 days").
pub fn duration(secs: i64) -> String {
    let days = secs / 86400;
    if days >= 365 {
        let y = days / 365;
        let d = days % 365;
        let ys = if y == 1 {
            "1 year".to_string()
        } else {
            format!("{y} years")
        };
        if d == 0 { ys } else { format!("{ys} {d} days") }
    } else if days > 0 {
        if days == 1 {
            "1 day".into()
        } else {
            format!("{days} days")
        }
    } else {
        format!("{secs} seconds")
    }
}

/// Whether the bytes look like text.
pub fn text(bytes: &[u8]) -> Option<&str> {
    let s = std::str::from_utf8(bytes).ok()?;
    let s = s.strip_prefix('\u{FEFF}').unwrap_or(s);
    (!s.contains('\0')).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_760_000_000), "2025-10-09 08:53:20 UTC");
        assert_eq!(parse_iso("2025-10-09T08:53:20Z"), Some(1_760_000_000));
    }
}
