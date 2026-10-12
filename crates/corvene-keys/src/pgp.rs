//! OpenPGP keys (RFC 4880 / RFC 9580, LibrePGP v5): armored or binary
//! packets. Signatures are read for their subpackets, not verified. Secret
//! key packets are read up to their public part plus the protection octet.

use crate::{der, oid, util};
use corvene_models::signing::{SigningFile, SigningItem, SigningItemKind, SigningKind};
use std::borrow::Cow;

/// Most packets read from one file.
const MAX_PACKETS: usize = 500_000;

struct Packet<'a> {
    tag: u8,
    body: Cow<'a, [u8]>,
}

/// Splits binary data into packets. Stops at the first malformed header.
fn packets(data: &[u8]) -> (Vec<Packet<'_>>, bool) {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() && out.len() < MAX_PACKETS {
        let h = data[pos];
        if h & 0x80 == 0 {
            return (out, true);
        }
        pos += 1;
        if h & 0x40 != 0 {
            let tag = h & 0x3F;
            let mut chunks: Vec<&[u8]> = Vec::new();
            loop {
                let Some(&o1) = data.get(pos) else {
                    return (out, true);
                };
                pos += 1;
                let (len, partial) = match o1 {
                    0..=191 => (usize::from(o1), false),
                    192..=223 => {
                        let Some(&o2) = data.get(pos) else {
                            return (out, true);
                        };
                        pos += 1;
                        (
                            ((usize::from(o1) - 192) << 8) + usize::from(o2) + 192,
                            false,
                        )
                    }
                    255 => {
                        let Some(b) = data.get(pos..pos + 4) else {
                            return (out, true);
                        };
                        pos += 4;
                        (u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize, false)
                    }
                    _ => (1usize << (o1 & 0x1F), true),
                };
                let Some(chunk) = pos.checked_add(len).and_then(|e| data.get(pos..e)) else {
                    return (out, true);
                };
                pos += len;
                chunks.push(chunk);
                if !partial {
                    break;
                }
            }
            let body = if chunks.len() == 1 {
                Cow::Borrowed(chunks[0])
            } else {
                Cow::Owned(chunks.concat())
            };
            out.push(Packet { tag, body });
        } else {
            let tag = (h >> 2) & 0x0F;
            let len = match h & 3 {
                0 => data.get(pos).map(|&b| (usize::from(b), 1)),
                1 => data
                    .get(pos..pos + 2)
                    .map(|b| (usize::from(u16::from_be_bytes([b[0], b[1]])), 2)),
                2 => data
                    .get(pos..pos + 4)
                    .map(|b| (u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize, 4)),
                _ => Some((data.len() - pos, 0)),
            };
            let Some((len, n)) = len else {
                return (out, true);
            };
            pos += n;
            let Some(body) = pos.checked_add(len).and_then(|e| data.get(pos..e)) else {
                return (out, true);
            };
            pos += len;
            out.push(Packet {
                tag,
                body: Cow::Borrowed(body),
            });
        }
    }
    (out, false)
}

fn pub_algo(a: u8) -> String {
    match a {
        1 => "RSA".into(),
        2 => "RSA (encrypt only)".into(),
        3 => "RSA (sign only)".into(),
        16 => "ElGamal".into(),
        17 => "DSA".into(),
        18 => "ECDH".into(),
        19 => "ECDSA".into(),
        20 => "ElGamal (sign and encrypt)".into(),
        22 => "EdDSA (legacy)".into(),
        25 => "X25519".into(),
        26 => "X448".into(),
        27 => "Ed25519".into(),
        28 => "Ed448".into(),
        n => format!("algorithm {n}"),
    }
}

fn sym_algo(a: u8) -> String {
    match a {
        1 => "IDEA".into(),
        2 => "3DES".into(),
        3 => "CAST5".into(),
        4 => "Blowfish".into(),
        7 => "AES-128".into(),
        8 => "AES-192".into(),
        9 => "AES-256".into(),
        10 => "Twofish".into(),
        11 => "Camellia-128".into(),
        12 => "Camellia-192".into(),
        13 => "Camellia-256".into(),
        n => format!("cipher {n}"),
    }
}

fn hash_algo(a: u8) -> String {
    match a {
        1 => "MD5".into(),
        2 => "SHA-1".into(),
        3 => "RIPEMD-160".into(),
        8 => "SHA-256".into(),
        9 => "SHA-384".into(),
        10 => "SHA-512".into(),
        11 => "SHA-224".into(),
        12 => "SHA3-256".into(),
        14 => "SHA3-512".into(),
        n => format!("hash {n}"),
    }
}

fn comp_algo(a: u8) -> String {
    match a {
        0 => "uncompressed".into(),
        1 => "ZIP".into(),
        2 => "ZLIB".into(),
        3 => "BZip2".into(),
        n => format!("compression {n}"),
    }
}

fn aead_algo(a: u8) -> String {
    match a {
        1 => "EAX".into(),
        2 => "OCB".into(),
        3 => "GCM".into(),
        n => format!("AEAD {n}"),
    }
}

fn sig_type(t: u8) -> &'static str {
    match t {
        0x00 => "binary document",
        0x01 => "text document",
        0x02 => "standalone",
        0x10 => "generic certification",
        0x11 => "persona certification",
        0x12 => "casual certification",
        0x13 => "positive certification",
        0x18 => "subkey binding",
        0x19 => "primary key binding",
        0x1F => "direct key",
        0x20 => "key revocation",
        0x28 => "subkey revocation",
        0x30 => "certification revocation",
        0x40 => "timestamp",
        0x50 => "third-party confirmation",
        _ => "unknown",
    }
}

fn mpi(data: &[u8], pos: &mut usize) -> Option<usize> {
    let b = data.get(*pos..*pos + 2)?;
    let bits = usize::from(u16::from_be_bytes([b[0], b[1]]));
    let len = bits.div_ceil(8);
    data.get(*pos + 2..*pos + 2 + len)?;
    *pos += 2 + len;
    Some(bits)
}

fn curve(data: &[u8], pos: &mut usize) -> Option<String> {
    let n = usize::from(*data.get(*pos)?);
    let o = data.get(*pos + 1..*pos + 1 + n)?;
    *pos += 1 + n;
    let dotted = der::oid_string(o)?;
    Some(oid::label(&dotted))
}

/// Parses algorithm-specific public fields from `pos`; returns the size.
fn public_material(algo: u8, data: &[u8], pos: &mut usize) -> Option<String> {
    Some(match algo {
        1..=3 => {
            let bits = mpi(data, pos)?;
            mpi(data, pos)?;
            format!("{bits} bits")
        }
        17 => {
            let bits = mpi(data, pos)?;
            for _ in 0..3 {
                mpi(data, pos)?;
            }
            format!("{bits} bits")
        }
        16 | 20 => {
            let bits = mpi(data, pos)?;
            mpi(data, pos)?;
            mpi(data, pos)?;
            format!("{bits} bits")
        }
        18 => {
            let c = curve(data, pos)?;
            mpi(data, pos)?;
            let n = usize::from(*data.get(*pos)?);
            data.get(*pos + 1..*pos + 1 + n)?;
            *pos += 1 + n;
            c
        }
        19 | 22 => {
            let c = curve(data, pos)?;
            mpi(data, pos)?;
            c
        }
        25 | 27 => {
            data.get(*pos..*pos + 32)?;
            *pos += 32;
            "256 bits".into()
        }
        26 => {
            data.get(*pos..*pos + 56)?;
            *pos += 56;
            "448 bits".into()
        }
        28 => {
            data.get(*pos..*pos + 57)?;
            *pos += 57;
            "456 bits".into()
        }
        _ => return None,
    })
}

#[derive(Clone)]
struct KeyPkt {
    version: u8,
    created: u32,
    algo: u8,
    size: Option<String>,
    fingerprint: Vec<u8>,
    key_id: Vec<u8>,
    /// For secret key packets: how the secret part is stored.
    secret: Option<&'static str>,
    v3_days: Option<u16>,
}

fn parse_key(body: &[u8], secret: bool) -> Option<KeyPkt> {
    let version = *body.first()?;
    let c = body.get(1..5)?;
    let created = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
    let mut k = KeyPkt {
        version,
        created,
        algo: 0,
        size: None,
        fingerprint: Vec::new(),
        key_id: Vec::new(),
        secret: None,
        v3_days: None,
    };
    let public_end;
    match version {
        2 | 3 => {
            let d = body.get(5..7)?;
            k.v3_days = Some(u16::from_be_bytes([d[0], d[1]]));
            k.algo = *body.get(7)?;
            let mut pos = 8;
            let n_start = pos;
            let bits = mpi(body, &mut pos)?;
            let n = body.get(n_start + 2..pos)?;
            let e_start = pos;
            mpi(body, &mut pos)?;
            let e = body.get(e_start + 2..pos)?;
            k.size = Some(format!("{bits} bits"));
            let mut m = n.to_vec();
            m.extend_from_slice(e);
            k.fingerprint = util::md5(&m).to_vec();
            k.key_id = n.get(n.len().saturating_sub(8)..)?.to_vec();
            public_end = pos;
        }
        4 => {
            k.algo = *body.get(5)?;
            let mut pos = 6;
            k.size = public_material(k.algo, body, &mut pos);
            if k.size.is_none() {
                // Unknown algorithm: only a public key packet has a known end.
                if secret {
                    return None;
                }
                pos = body.len();
            }
            public_end = pos;
            let mut h = vec![0x99];
            h.extend_from_slice(&(public_end as u16).to_be_bytes());
            h.extend_from_slice(body.get(..public_end)?);
            k.fingerprint = util::sha1(&h).to_vec();
            k.key_id = k.fingerprint[12..].to_vec();
        }
        5 | 6 => {
            k.algo = *body.get(5)?;
            let l = body.get(6..10)?;
            let len = u32::from_be_bytes([l[0], l[1], l[2], l[3]]) as usize;
            public_end = 10usize.checked_add(len)?;
            let material = body.get(10..public_end)?;
            let mut pos = 0;
            k.size = public_material(k.algo, material, &mut pos);
            let mut h = vec![if version == 6 { 0x9B } else { 0x9A }];
            h.extend_from_slice(&(public_end as u32).to_be_bytes());
            h.extend_from_slice(body.get(..public_end)?);
            k.fingerprint = util::sha256(&h).to_vec();
            k.key_id = k.fingerprint[..8].to_vec();
        }
        _ => return None,
    }
    if secret {
        let usage = body.get(public_end).copied();
        k.secret = Some(match usage {
            None => "secret key material missing",
            Some(0) => "secret key material present (unprotected)",
            Some(254 | 255) if version == 4 && body.get(public_end + 2) == Some(&101) => {
                "secret key stub (material not in this file, e.g. on a smartcard)"
            }
            Some(_) => "secret key material present (protected)",
        });
    }
    Some(k)
}

#[derive(Clone, Default)]
struct Sig {
    version: u8,
    sig_type: u8,
    pub_algo: u8,
    hash_algo: u8,
    created: Option<u32>,
    key_expiry: Option<u32>,
    sig_expiry: Option<u32>,
    flags: Option<u8>,
    primary_uid: bool,
    pref_sym: Vec<u8>,
    pref_hash: Vec<u8>,
    pref_comp: Vec<u8>,
    pref_aead: Vec<u8>,
    /// Preferred AEAD ciphersuites (v6: cipher and mode pairs).
    pref_suites: Vec<u8>,
    revocation: Option<(u8, String)>,
    issuer_id: Option<Vec<u8>>,
    issuer_fp: Option<Vec<u8>>,
}

fn subpackets(data: &[u8], sig: &mut Sig, hashed: bool) {
    let mut pos = 0;
    while pos < data.len() {
        let o1 = data[pos];
        let (len, n) = match o1 {
            0..=191 => (usize::from(o1), 1),
            192..=254 => match data.get(pos + 1) {
                Some(&o2) => (((usize::from(o1) - 192) << 8) + usize::from(o2) + 192, 2),
                None => return,
            },
            255 => match data.get(pos + 1..pos + 5) {
                Some(b) => (u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize, 5),
                None => return,
            },
        };
        pos += n;
        let Some(sp) = pos.checked_add(len).and_then(|e| data.get(pos..e)) else {
            return;
        };
        pos += len;
        let Some((&ty, v)) = sp.split_first() else {
            continue;
        };
        let u32v = || {
            v.get(..4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        match (ty & 0x7F, hashed) {
            (2, true) => sig.created = u32v(),
            (3, true) => sig.sig_expiry = u32v(),
            (9, true) => sig.key_expiry = u32v(),
            (11, true) => sig.pref_sym = v.to_vec(),
            (21, true) => sig.pref_hash = v.to_vec(),
            (22, true) => sig.pref_comp = v.to_vec(),
            (39, true) => sig.pref_suites = v.to_vec(),
            (34, true) => sig.pref_aead = v.to_vec(),
            (25, true) => sig.primary_uid = v.first().is_some_and(|&b| b != 0),
            (27, true) => sig.flags = v.first().copied(),
            (29, true) => {
                if let Some((&code, reason)) = v.split_first() {
                    sig.revocation = Some((code, String::from_utf8_lossy(reason).into_owned()));
                }
            }
            (16, _) => sig.issuer_id = Some(v.to_vec()),
            (33, _) => sig.issuer_fp = v.get(1..).map(<[u8]>::to_vec),
            _ => {}
        }
    }
}

fn parse_sig(body: &[u8]) -> Option<Sig> {
    let version = *body.first()?;
    let mut s = Sig {
        version,
        ..Sig::default()
    };
    match version {
        3 | 2 => {
            s.sig_type = *body.get(2)?;
            let c = body.get(3..7)?;
            s.created = Some(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
            s.issuer_id = Some(body.get(7..15)?.to_vec());
            s.pub_algo = *body.get(15)?;
            s.hash_algo = *body.get(16)?;
        }
        4..=6 => {
            s.sig_type = *body.get(1)?;
            s.pub_algo = *body.get(2)?;
            s.hash_algo = *body.get(3)?;
            let (len_size, mut pos) = if version == 6 { (4, 4) } else { (2, 4) };
            let read_len = |p: usize| -> Option<usize> {
                let b = body.get(p..p + len_size)?;
                Some(b.iter().fold(0usize, |a, &x| (a << 8) | usize::from(x)))
            };
            let hl = read_len(pos)?;
            pos += len_size;
            let hashed = body.get(pos..pos.checked_add(hl)?)?;
            subpackets(hashed, &mut s, true);
            pos += hl;
            if let Some(ul) = read_len(pos) {
                pos += len_size;
                if let Some(unhashed) = pos.checked_add(ul).and_then(|e| body.get(pos..e)) {
                    subpackets(unhashed, &mut s, false);
                }
            }
        }
        _ => return None,
    }
    Some(s)
}

fn fp_text(fp: &[u8]) -> String {
    let h = util::hex(fp).to_uppercase();
    h.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn flags_text(f: u8) -> String {
    let names = [
        (0x01, "certify"),
        (0x02, "sign"),
        (0x04, "encrypt communications"),
        (0x08, "encrypt storage"),
        (0x10, "split key"),
        (0x20, "authenticate"),
        (0x80, "shared"),
    ];
    let v: Vec<&str> = names
        .iter()
        .filter(|(b, _)| f & b != 0)
        .map(|(_, n)| *n)
        .collect();
    if v.is_empty() {
        "none".into()
    } else {
        v.join(", ")
    }
}

fn reason_text(code: u8, text: &str) -> String {
    let r = match code {
        0 => "no reason",
        1 => "superseded",
        2 => "compromised",
        3 => "retired",
        32 => "user ID no longer valid",
        _ => "other",
    };
    if text.is_empty() {
        r.into()
    } else {
        format!("{r}: {text}")
    }
}

#[derive(Default)]
struct Uid {
    text: String,
    attribute: bool,
    size: usize,
    sigs: Vec<Sig>,
}

#[derive(Default)]
struct Sub {
    key: Option<KeyPkt>,
    sigs: Vec<Sig>,
}

#[derive(Default)]
struct Cert {
    key: Option<KeyPkt>,
    direct: Vec<Sig>,
    uids: Vec<Uid>,
    subs: Vec<Sub>,
}

enum Target {
    Primary,
    Uid,
    Sub,
}

fn is_self(sig: &Sig, key: &KeyPkt) -> bool {
    match (&sig.issuer_fp, &sig.issuer_id) {
        (Some(fp), _) => *fp == key.fingerprint,
        (None, Some(id)) => *id == key.key_id,
        (None, None) => true,
    }
}

fn latest<'a>(sigs: impl Iterator<Item = &'a Sig>) -> Option<&'a Sig> {
    sigs.max_by_key(|s| s.created.unwrap_or(0))
}

fn key_fields(item: &mut SigningItem, k: &KeyPkt, binding: Option<&Sig>, revoked: Option<&Sig>) {
    item.field("Version", k.version.to_string());
    item.field("Algorithm", pub_algo(k.algo));
    if let Some(s) = &k.size {
        item.field("Size", s.clone());
    }
    item.field("Created", util::utc(i64::from(k.created)));
    item.not_before = Some(i64::from(k.created));
    let expiry = binding
        .and_then(|b| b.key_expiry)
        .filter(|&e| e > 0)
        .map(i64::from)
        .or_else(|| k.v3_days.filter(|&d| d > 0).map(|d| i64::from(d) * 86400));
    match expiry {
        Some(e) => {
            let at = i64::from(k.created) + e;
            item.field("Expires", util::utc(at));
            item.not_after = Some(at);
        }
        None => item.field("Expires", "never"),
    }
    item.field("Fingerprint", fp_text(&k.fingerprint));
    item.field("Key ID", util::hex(&k.key_id).to_uppercase());
    if let Some(f) = binding.and_then(|b| b.flags) {
        item.field("Usage", flags_text(f));
    }
    if let Some(s) = k.secret {
        item.field("Secret key", s);
    }
    if let Some(r) = revoked {
        let (code, text) = r.revocation.clone().unwrap_or((0, String::new()));
        let when = r
            .created
            .map(|c| format!(" on {}", util::utc(i64::from(c))))
            .unwrap_or_default();
        item.field(
            "Revoked",
            format!("yes{when} ({})", reason_text(code, &text)),
        );
    }
}

fn prefs(item: &mut SigningItem, s: &Sig) {
    let join =
        |v: &[u8], f: fn(u8) -> String| v.iter().map(|&a| f(a)).collect::<Vec<_>>().join(", ");
    if !s.pref_sym.is_empty() {
        item.field("Preferred ciphers", join(&s.pref_sym, sym_algo));
    }
    if !s.pref_hash.is_empty() {
        item.field("Preferred hashes", join(&s.pref_hash, hash_algo));
    }
    if !s.pref_comp.is_empty() {
        item.field("Preferred compression", join(&s.pref_comp, comp_algo));
    }
    if !s.pref_aead.is_empty() {
        item.field("Preferred AEAD", join(&s.pref_aead, aead_algo));
    }
    if !s.pref_suites.is_empty() {
        let suites: Vec<String> = s
            .pref_suites
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[c, m]| format!("{}/{}", sym_algo(*c), aead_algo(*m)))
            .collect();
        item.field("Preferred AEAD ciphersuites", suites.join(", "));
    }
}

fn cert_item(c: &Cert) -> Option<SigningItem> {
    let k = c.key.as_ref()?;
    let self_sigs =
        |sigs: &[Sig]| -> Vec<Sig> { sigs.iter().filter(|s| is_self(s, k)).cloned().collect() };
    let direct = self_sigs(&c.direct);
    let revoked = direct.iter().find(|s| s.sig_type == 0x20);
    // Primary user ID: flagged in its latest self certification, else first.
    let uid_latest: Vec<Option<Sig>> = c
        .uids
        .iter()
        .map(|u| {
            latest(
                self_sigs(&u.sigs)
                    .iter()
                    .filter(|s| (0x10..=0x13).contains(&s.sig_type)),
            )
            .cloned()
        })
        .collect();
    let primary = c
        .uids
        .iter()
        .enumerate()
        .find(|(i, u)| !u.attribute && uid_latest[*i].as_ref().is_some_and(|s| s.primary_uid))
        .or_else(|| c.uids.iter().enumerate().find(|(_, u)| !u.attribute))
        .map(|(i, _)| i);
    let binding = latest(
        direct
            .iter()
            .filter(|s| s.sig_type == 0x1F)
            .chain(primary.and_then(|i| uid_latest[i].as_ref())),
    );
    let fp = util::hex(&k.fingerprint).to_uppercase();
    let title = primary
        .and_then(|i| c.uids.get(i))
        .map(|u| u.text.clone())
        .unwrap_or_else(|| format!("Key {}", util::hex(&k.key_id).to_uppercase()));
    let mut item = SigningItem::new(SigningItemKind::PgpKey, format!("pgp:{fp}"), title);
    key_fields(&mut item, k, binding, revoked);
    if let Some(b) = binding {
        prefs(&mut item, b);
    }
    item.field(
        "User IDs",
        c.uids.iter().filter(|u| !u.attribute).count().to_string(),
    );
    let attrs = c.uids.iter().filter(|u| u.attribute).count();
    if attrs > 0 {
        item.field("User attributes", attrs.to_string());
    }
    item.field("Subkeys", c.subs.len().to_string());
    let mut photo = 0;
    for (i, u) in c.uids.iter().enumerate() {
        let (key, title, kind) = if u.attribute {
            photo += 1;
            (
                format!("uattr:{photo}"),
                format!("User attribute {photo}"),
                SigningItemKind::PgpUserAttribute,
            )
        } else {
            (
                format!("uid:{}", u.text),
                u.text.clone(),
                SigningItemKind::PgpUserId,
            )
        };
        let mut child = SigningItem::new(kind, key, title);
        if u.attribute {
            child.field("Size", format!("{} bytes", u.size));
        }
        child.field("Primary", if primary == Some(i) { "yes" } else { "no" });
        if let Some(t) = uid_latest[i].as_ref().and_then(|s| s.created) {
            child.field("Self-certified", util::utc(i64::from(t)));
        }
        let own = self_sigs(&u.sigs);
        if let Some(r) = own.iter().find(|s| s.sig_type == 0x30) {
            let (code, text) = r.revocation.clone().unwrap_or((0, String::new()));
            child.field("Revoked", reason_text(code, &text));
        }
        let third = u.sigs.len() - own.len();
        if third > 0 {
            child.field("Third-party certifications", third.to_string());
        }
        item.children.push(child);
    }
    for s in &c.subs {
        let Some(sk) = &s.key else { continue };
        let own = self_sigs(&s.sigs);
        let bind = latest(own.iter().filter(|x| x.sig_type == 0x18));
        let rev = own.iter().find(|x| x.sig_type == 0x28);
        let sfp = util::hex(&sk.fingerprint).to_uppercase();
        let mut child = SigningItem::new(
            SigningItemKind::PgpSubkey,
            format!("pgp:{sfp}"),
            format!("Subkey {}", util::hex(&sk.key_id).to_uppercase()),
        );
        key_fields(&mut child, sk, bind, rev);
        item.children.push(child);
    }
    Some(item)
}

fn sig_item(s: &Sig, n: usize) -> SigningItem {
    let issuer = s
        .issuer_fp
        .as_ref()
        .map(|f| fp_text(f))
        .or_else(|| s.issuer_id.as_ref().map(|i| util::hex(i).to_uppercase()));
    let mut item = SigningItem::new(
        SigningItemKind::PgpSignature,
        format!("sig:{n}"),
        format!("Signature ({})", sig_type(s.sig_type)),
    );
    item.field("Version", s.version.to_string());
    item.field("Type", sig_type(s.sig_type));
    item.field("Algorithm", pub_algo(s.pub_algo));
    item.field("Hash", hash_algo(s.hash_algo));
    if let Some(c) = s.created {
        item.field("Created", util::utc(i64::from(c)));
    }
    if let Some(i) = issuer {
        item.field("Issuer", i);
    }
    item
}

fn packet_name(tag: u8) -> String {
    match tag {
        1 => "public-key encrypted session key".into(),
        2 => "signature".into(),
        3 => "symmetric-key encrypted session key".into(),
        4 => "one-pass signature".into(),
        8 => "compressed data".into(),
        9 => "symmetrically encrypted data".into(),
        11 => "literal data".into(),
        18 => "encrypted and integrity protected data".into(),
        20 => "AEAD encrypted data".into(),
        n => format!("packet {n}"),
    }
}

/// Reads binary packets. `None` when no packet was understood.
pub fn read_binary(data: &[u8]) -> Option<SigningFile> {
    let (pkts, truncated) = packets(data);
    let mut certs: Vec<Cert> = Vec::new();
    let mut loose_sigs = Vec::new();
    let mut message = Vec::new();
    let mut target = Target::Primary;
    let mut secret = false;
    for p in &pkts {
        match p.tag {
            5 | 6 => {
                secret |= p.tag == 5;
                certs.push(Cert {
                    key: parse_key(&p.body, p.tag == 5),
                    ..Cert::default()
                });
                target = Target::Primary;
            }
            7 | 14 => {
                if let Some(c) = certs.last_mut() {
                    c.subs.push(Sub {
                        key: parse_key(&p.body, p.tag == 7),
                        sigs: Vec::new(),
                    });
                    target = Target::Sub;
                }
            }
            13 | 17 => {
                if let Some(c) = certs.last_mut() {
                    // Only the size of a user attribute (photo) is kept.
                    let text = if p.tag == 13 {
                        String::from_utf8_lossy(&p.body).into_owned()
                    } else {
                        String::new()
                    };
                    c.uids.push(Uid {
                        text,
                        attribute: p.tag == 17,
                        size: p.body.len(),
                        sigs: Vec::new(),
                    });
                    target = Target::Uid;
                }
            }
            2 => {
                let Some(s) = parse_sig(&p.body) else {
                    continue;
                };
                match certs.last_mut() {
                    Some(c) => match target {
                        Target::Primary => c.direct.push(s),
                        Target::Uid => {
                            if let Some(u) = c.uids.last_mut() {
                                u.sigs.push(s);
                            }
                        }
                        Target::Sub => {
                            if let Some(sub) = c.subs.last_mut() {
                                sub.sigs.push(s);
                            }
                        }
                    },
                    None => loose_sigs.push(s),
                }
            }
            12 => {}
            t => message.push(t),
        }
    }
    let mut file = SigningFile::default();
    file.items.extend(certs.iter().filter_map(cert_item));
    for (n, s) in loose_sigs.iter().enumerate() {
        file.items.push(sig_item(s, n));
    }
    if !message.is_empty() {
        let mut item = SigningItem::new(SigningItemKind::PgpMessage, "message", "OpenPGP message");
        let mut names: Vec<String> = Vec::new();
        for t in &message {
            let n = packet_name(*t);
            if !names.contains(&n) {
                names.push(n);
            }
        }
        item.field("Packets", names.join(", "));
        for p in pkts.iter().filter(|p| p.tag == 1) {
            if let Some(id) = p.body.get(1..9).filter(|_| p.body.first() == Some(&3)) {
                item.field("Recipient key ID", util::hex(id).to_uppercase());
            }
        }
        file.items.push(item);
    }
    if file.items.is_empty() {
        return None;
    }
    file.kind = if !certs.is_empty() {
        if secret {
            SigningKind::PgpSecretKeys
        } else {
            SigningKind::PgpPublicKeys
        }
    } else if !message.is_empty() {
        SigningKind::PgpMessage
    } else {
        SigningKind::PgpSignature
    };
    file.notes
        .push("Signatures are read but not verified cryptographically".into());
    if secret {
        file.notes
            .push("Secret key material is never decoded or shown".into());
    }
    if truncated {
        file.notes
            .push("The data ends early or has an unreadable packet".into());
    }
    Some(file)
}

/// Base64 bodies of armored blocks: (label, bytes).
pub fn dearmor(text: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        let Some(label) = line
            .strip_prefix("-----BEGIN PGP ")
            .and_then(|l| l.strip_suffix("-----"))
        else {
            continue;
        };
        if label == "SIGNED MESSAGE" {
            out.push((label.to_string(), Vec::new()));
            continue;
        }
        let end = format!("-----END PGP {label}-----");
        let mut body: Vec<&str> = Vec::new();
        for l in lines.by_ref() {
            let l = l.trim();
            if l == end {
                break;
            }
            body.push(l);
        }
        // Headers end at the first blank line.
        let start = match body.iter().position(|l| l.is_empty()) {
            Some(p) if body[..p].iter().all(|l| l.contains(':')) => p + 1,
            _ => body.iter().take_while(|l| l.contains(": ")).count(),
        };
        let b64: String = body[start.min(body.len())..]
            .iter()
            .filter(|l| !(l.starts_with('=') && l.len() == 5))
            .copied()
            .collect();
        if let Some(bytes) = util::b64(&b64) {
            out.push((label.to_string(), bytes));
        }
    }
    out
}

/// Reads armored text. `None` when nothing was understood.
pub fn read_armored(text: &str) -> Option<SigningFile> {
    let blocks = dearmor(text);
    let mut file: Option<SigningFile> = None;
    let mut cleartext = false;
    for (label, bytes) in blocks {
        if label == "SIGNED MESSAGE" {
            cleartext = true;
            continue;
        }
        let Some(f) = read_binary(&bytes) else {
            continue;
        };
        match &mut file {
            None => file = Some(f),
            Some(acc) => {
                acc.items.extend(f.items);
                for n in f.notes {
                    if !acc.notes.contains(&n) {
                        acc.notes.push(n);
                    }
                }
                if f.kind == SigningKind::PgpSecretKeys {
                    acc.kind = f.kind;
                }
            }
        }
    }
    if cleartext {
        let f = file.get_or_insert_with(SigningFile::default);
        f.kind = SigningKind::PgpSignature;
        f.items.insert(0, {
            let mut i = SigningItem::new(
                SigningItemKind::PgpMessage,
                "message",
                "Cleartext signed message",
            );
            i.field("Type", "cleartext signed message");
            i
        });
    }
    file
}
