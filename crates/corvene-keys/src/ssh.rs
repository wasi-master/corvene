//! SSH public keys, certificates, `authorized_keys`, `known_hosts`,
//! `allowed_signers`, RFC 4716 blocks and the public part of OpenSSH
//! private keys.

use crate::{oid, util};
use corvene_models::signing::{SigningFile, SigningItem, SigningItemKind, SigningKind};

/// Most lines read from one file.
const MAX_LINES: usize = 200_000;

struct Wire<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Wire<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Some(u64::from_be_bytes(a))
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn text(&mut self) -> Option<String> {
        self.string()
            .map(|s| String::from_utf8_lossy(s).into_owned())
    }
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

const CERT_SUFFIX: &str = "-cert-v01@openssh.com";

/// Algorithm-specific public fields: (size text, security key application).
fn key_fields(ty: &str, w: &mut Wire) -> Option<(Option<String>, Option<String>)> {
    Some(match ty {
        "ssh-rsa" => {
            let _e = w.string()?;
            let n = w.string()?;
            (Some(format!("{} bits", crate::der::bit_len(n))), None)
        }
        "ssh-dss" => {
            let p = w.string()?;
            for _ in 0..3 {
                w.string()?;
            }
            (Some(format!("{} bits", crate::der::bit_len(p))), None)
        }
        "ssh-ed25519" => {
            w.string()?;
            (Some("256 bits".into()), None)
        }
        "ssh-ed448" => {
            w.string()?;
            (Some("456 bits".into()), None)
        }
        t if t.starts_with("ecdsa-sha2-") => {
            let curve = w.text()?;
            w.string()?;
            (Some(curve_size(&curve)), None)
        }
        "sk-ecdsa-sha2-nistp256@openssh.com" => {
            let curve = w.text()?;
            w.string()?;
            let app = w.text()?;
            (Some(curve_size(&curve)), Some(app))
        }
        "sk-ssh-ed25519@openssh.com" => {
            w.string()?;
            let app = w.text()?;
            (Some("256 bits".into()), Some(app))
        }
        _ => (None, None),
    })
}

fn curve_size(curve: &str) -> String {
    let name = match curve {
        "nistp256" => "P-256",
        "nistp384" => "P-384",
        "nistp521" => "P-521",
        other => other,
    };
    match oid::curve_bits(name) {
        Some(b) => format!("{name} ({b} bits)"),
        None => name.to_string(),
    }
}

fn sha256_fp(blob: &[u8]) -> String {
    format!("SHA256:{}", util::b64_nopad(&util::sha256(blob)))
}

fn md5_fp(blob: &[u8]) -> String {
    format!("MD5:{}", util::colon_hex(&util::md5(blob), false))
}

/// Packed name/value list of certificate options.
fn options(data: &[u8]) -> String {
    let mut w = Wire::new(data);
    let mut out = Vec::new();
    while w.pos < data.len() {
        let (Some(name), Some(value)) = (w.text(), w.string()) else {
            break;
        };
        if value.is_empty() {
            out.push(name);
        } else {
            let inner = Wire::new(value).text().unwrap_or_else(|| util::hex(value));
            out.push(format!("{name}={inner}"));
        }
    }
    out.join(", ")
}

fn cert_time(t: u64) -> String {
    if t == u64::MAX {
        "forever".into()
    } else {
        util::utc(i64::try_from(t).unwrap_or(i64::MAX))
    }
}

/// A parsed key blob.
struct Key {
    ty: String,
    item: SigningItem,
    fingerprint: String,
    is_cert: bool,
}

/// Reads a public key or certificate blob into an item.
fn key(blob: &[u8]) -> Option<Key> {
    let mut w = Wire::new(blob);
    let ty = w.text()?;
    let mut item = SigningItem::new(SigningItemKind::SshKey, String::new(), String::new());
    let is_cert = ty.ends_with(CERT_SUFFIX);
    let fingerprint;
    if is_cert {
        let plain = ty.trim_end_matches(CERT_SUFFIX).to_string();
        let plain = if plain.starts_with("sk-") {
            format!("{plain}@openssh.com")
        } else {
            plain
        };
        w.string()?;
        let start = w.pos;
        let (size, app) = key_fields(&plain, &mut w)?;
        let mut plain_blob = Vec::new();
        put_string(&mut plain_blob, plain.as_bytes());
        plain_blob.extend_from_slice(blob.get(start..w.pos)?);
        fingerprint = sha256_fp(&plain_blob);
        let serial = w.u64()?;
        let cert_type = w.u32()?;
        let key_id = w.text()?;
        let principals_raw = w.string()?;
        let after = w.u64()?;
        let before = w.u64()?;
        let critical = w.string()?;
        let extensions = w.string()?;
        let _reserved = w.string()?;
        let ca = w.string()?;
        let mut p = Wire::new(principals_raw);
        let mut principals = Vec::new();
        while let Some(s) = p.text() {
            principals.push(s);
        }
        item.kind = SigningItemKind::SshCertificate;
        item.field("Type", ty.clone());
        item.field("Key type", plain);
        if let Some(s) = size {
            item.field("Size", s);
        }
        if let Some(a) = app {
            item.field("Application", a);
        }
        item.field("Fingerprint", fingerprint.clone());
        item.field("Key ID", key_id);
        item.field("Serial", serial.to_string());
        item.field(
            "Certificate type",
            match cert_type {
                1 => "user".to_string(),
                2 => "host".to_string(),
                n => n.to_string(),
            },
        );
        item.field(
            "Principals",
            if principals.is_empty() {
                "(any)".into()
            } else {
                principals.join(", ")
            },
        );
        item.field("Valid after", cert_time(after));
        item.field("Valid before", cert_time(before));
        if after > 0 {
            item.not_before = i64::try_from(after).ok();
        }
        if before != u64::MAX {
            item.not_after = i64::try_from(before).ok();
        }
        let crit = options(critical);
        item.field(
            "Critical options",
            if crit.is_empty() {
                "(none)".into()
            } else {
                crit
            },
        );
        item.field("Extensions", options(extensions));
        let ca_type = Wire::new(ca).text().unwrap_or_default();
        item.field("Signing CA", format!("{ca_type} {}", sha256_fp(ca)));
    } else {
        let (size, app) = key_fields(&ty, &mut w)?;
        fingerprint = sha256_fp(blob);
        item.field("Type", ty.clone());
        if let Some(s) = size {
            item.field("Size", s);
        }
        if let Some(a) = app {
            item.field("Application", a);
        }
        item.field("Fingerprint", fingerprint.clone());
        item.field("MD5 fingerprint", md5_fp(blob));
    }
    Some(Key {
        ty,
        item,
        fingerprint,
        is_cert,
    })
}

/// Splits a line into tokens, keeping double-quoted parts together.
fn tokens(line: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if quoted => escaped = true,
            '"' => {
                quoted = !quoted;
                start.get_or_insert(i);
            }
            c if c.is_whitespace() && !quoted => {
                if let Some(s) = start.take() {
                    out.push((s, i));
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push((s, line.len()));
    }
    out
}

fn looks_like_type(t: &str) -> bool {
    t.starts_with("ssh-") || t.starts_with("ecdsa-") || t.starts_with("sk-")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Authorized,
    KnownHosts,
    AllowedSigners,
}

/// One key line: the key and the text before and after it.
fn line_key(line: &str) -> Option<(Key, &str, &str)> {
    let toks = tokens(line);
    for i in 0..toks.len().saturating_sub(1) {
        let t = &line[toks[i].0..toks[i].1];
        if !looks_like_type(t) {
            continue;
        }
        let Some(blob) = util::b64(&line[toks[i + 1].0..toks[i + 1].1]) else {
            continue;
        };
        let Some(k) = key(&blob) else { continue };
        if k.ty != t {
            continue;
        }
        let prefix = line[..toks[i].0].trim();
        let comment = line[toks[i + 1].1..].trim();
        return Some((k, prefix, comment));
    }
    None
}

fn flavor_of(name: &str) -> Flavor {
    if name.contains("known_hosts") {
        Flavor::KnownHosts
    } else if name.contains("allowed_signers") {
        Flavor::AllowedSigners
    } else {
        Flavor::Authorized
    }
}

/// Reads SSH key lines. `None` when no line holds a key.
pub fn read_lines(name: &str, text: &str) -> Option<SigningFile> {
    let flavor = flavor_of(name);
    let mut file = SigningFile::default();
    let mut unread = 0usize;
    let mut certs = 0;
    for line in text.lines().take(MAX_LINES) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((mut k, prefix, comment)) = line_key(trimmed) else {
            unread += 1;
            continue;
        };
        if k.is_cert {
            certs += 1;
        }
        let item = &mut k.item;
        if !comment.is_empty() {
            item.field("Comment", comment);
        }
        let mut title = if comment.is_empty() {
            k.fingerprint.clone()
        } else {
            comment.to_string()
        };
        match flavor {
            Flavor::KnownHosts => {
                let (marker, hosts) = match prefix.split_once(char::is_whitespace) {
                    Some((m, h)) if m.starts_with('@') => (Some(m), h.trim()),
                    _ => (None, prefix),
                };
                if let Some(m) = marker {
                    item.field("Marker", m);
                }
                let shown = if hosts.starts_with("|1|") {
                    format!("hashed host ({hosts})")
                } else {
                    hosts.into()
                };
                item.field("Hosts", shown.clone());
                item.key = format!("host:{} {hosts} {}", marker.unwrap_or(""), k.ty);
                title = if hosts.starts_with("|1|") {
                    "Hashed host".into()
                } else {
                    hosts.to_string()
                };
                if let Some(m) = marker {
                    title = format!("{m} {title}");
                }
            }
            Flavor::AllowedSigners => {
                let toks = tokens(prefix);
                let principals = toks.first().map_or("", |t| &prefix[t.0..t.1]);
                let opts = toks.first().map_or("", |t| prefix[t.1..].trim());
                item.field("Principals", principals);
                if !opts.is_empty() {
                    item.field("Options", opts);
                }
                item.key = format!("signer:{principals}");
                title = principals.to_string();
            }
            Flavor::Authorized => {
                if !prefix.is_empty() {
                    item.field("Options", prefix);
                }
                item.key = if k.is_cert {
                    format!("sshcert:{}", item.value("Key ID").unwrap_or_default())
                } else {
                    format!("ssh:{}", k.fingerprint)
                };
            }
        }
        item.title = title;
        file.items.push(k.item);
    }
    if file.items.is_empty() {
        return None;
    }
    if unread > 0 {
        file.notes.push(format!("Lines not read as keys: {unread}"));
    }
    file.kind = match flavor {
        Flavor::KnownHosts => SigningKind::SshKnownHosts,
        Flavor::AllowedSigners => SigningKind::SshAllowedSigners,
        Flavor::Authorized if file.items.len() == 1 && certs == 1 => SigningKind::SshCertificate,
        Flavor::Authorized if file.items.len() == 1 && !name.contains("authorized_keys") => {
            SigningKind::SshPublicKey
        }
        Flavor::Authorized => SigningKind::SshAuthorizedKeys,
    };
    Some(file)
}

/// An RFC 4716 `---- BEGIN SSH2 PUBLIC KEY ----` block body (headers and
/// base64).
pub fn rfc4716(body: &str) -> Option<SigningItem> {
    let mut comment = None;
    let mut b64 = String::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if let Some((h, v)) = line.split_once(':') {
            let mut v = v.trim().to_string();
            while v.ends_with('\\') {
                v.pop();
                match lines.next() {
                    Some(n) => v.push_str(n.trim()),
                    None => break,
                }
            }
            if h.eq_ignore_ascii_case("comment") {
                comment = Some(v.trim_matches('"').to_string());
            }
        } else {
            b64.push_str(line);
        }
    }
    let blob = util::b64(&b64)?;
    let mut k = key(&blob)?;
    if let Some(c) = &comment {
        k.item.field("Comment", c.clone());
    }
    k.item.key = format!("ssh:{}", k.fingerprint);
    k.item.title = comment.unwrap_or_else(|| k.fingerprint.clone());
    Some(k.item)
}

/// An `openssh-key-v1` private key: the public keys and how the private
/// part is protected. The private section is never read.
pub fn openssh_private(data: &[u8]) -> Option<Vec<SigningItem>> {
    let rest = data.strip_prefix(b"openssh-key-v1\0")?;
    let mut w = Wire::new(rest);
    let cipher = w.text()?;
    let kdf = w.text()?;
    let kdf_opts = w.string()?;
    let n = w.u32()?;
    let protection = if cipher == "none" {
        "unencrypted".to_string()
    } else {
        let mut o = Wire::new(kdf_opts);
        let rounds = o.string().and_then(|_| o.u32());
        match rounds {
            Some(r) => format!("protected with {kdf} / {cipher}, {r} rounds"),
            None => format!("protected with {kdf} / {cipher}"),
        }
    };
    let mut items = Vec::new();
    for _ in 0..n.min(64) {
        let blob = w.string()?;
        let Some(k) = key(blob) else { continue };
        let mut item = SigningItem::new(
            SigningItemKind::PrivateKey,
            format!("private:{}", k.fingerprint),
            format!("{} private key", k.ty),
        );
        item.field("Private key", "present (not shown)");
        item.field("Protection", protection.clone());
        item.fields.extend(k.item.fields);
        items.push(item);
    }
    Some(items)
}
