//! X.509 certificates, signing requests, revocation lists, public keys and
//! the public halves of PEM / PKCS#8 private keys.

use crate::der::{self, Reader, Tlv};
use crate::{oid, util};
use corvene_models::signing::{SigningItem, SigningItemKind};

/// Most revoked entries listed for one CRL.
const MAX_REVOKED: usize = 5000;

/// A distinguished name.
#[derive(Clone, Debug, Default)]
pub struct Name {
    /// RFC 4514 string (most specific RDN first).
    pub text: String,
    pub cn: Option<String>,
    pub o: Vec<String>,
    pub ou: Vec<String>,
}

fn escape_rdn(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let count = v.chars().count();
    for (i, c) in v.chars().enumerate() {
        let edge = (i == 0 && (c == '#' || c == ' ')) || (i + 1 == count && c == ' ');
        if matches!(c, ',' | '+' | '"' | '\\' | '<' | '>' | ';') || edge {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Parses a Name (SEQUENCE OF SET OF AttributeTypeAndValue).
pub fn parse_name(t: &Tlv) -> Name {
    let mut name = Name::default();
    let mut rdns = Vec::new();
    for set in t.children() {
        let mut parts = Vec::new();
        for atv in set.children() {
            let mut r = atv.children();
            let (Some(o), Some(v)) = (r.next(), r.next()) else {
                continue;
            };
            let Some(dotted) = der::oid(&o) else {
                continue;
            };
            let value = der::string(&v).unwrap_or_else(|| format!("#{}", util::hex(v.raw)));
            match dotted.as_str() {
                "2.5.4.3" if name.cn.is_none() => name.cn = Some(value.clone()),
                "2.5.4.10" => name.o.push(value.clone()),
                "2.5.4.11" => name.ou.push(value.clone()),
                _ => {}
            }
            parts.push(format!("{}={}", oid::label(&dotted), escape_rdn(&value)));
        }
        if !parts.is_empty() {
            rdns.push(parts.join("+"));
        }
    }
    rdns.reverse();
    name.text = rdns.join(", ");
    name
}

/// What a public key is.
#[derive(Clone, Debug, Default)]
pub struct KeyInfo {
    /// "RSA", "EC", "Ed25519" ...
    pub algorithm: String,
    /// "2048 bits", "P-256 (256 bits)".
    pub size: Option<String>,
    /// SHA-256 of the DER SubjectPublicKeyInfo, colon hex.
    pub spki_sha256: String,
    /// OpenSSH SHA256 fingerprint when the key type exists in SSH.
    pub ssh_sha256: Option<String>,
}

impl KeyInfo {
    /// "RSA 2048 bits".
    pub fn summary(&self) -> String {
        match &self.size {
            Some(s) => format!("{} {}", self.algorithm, s),
            None => self.algorithm.clone(),
        }
    }

    pub fn add_fields(&self, item: &mut SigningItem) {
        item.field("Public key", self.summary());
        item.field("Public key SHA-256", self.spki_sha256.clone());
        if let Some(s) = &self.ssh_sha256 {
            item.field("SSH fingerprint", s.clone());
        }
    }
}

fn ssh_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

fn ssh_mpint(out: &mut Vec<u8>, mut v: &[u8]) {
    while let [0, rest @ ..] = v {
        v = rest;
    }
    if v.first().is_some_and(|b| b & 0x80 != 0) {
        let mut p = vec![0];
        p.extend_from_slice(v);
        ssh_string(out, &p);
    } else {
        ssh_string(out, v);
    }
}

fn ssh_fp(blob: &[u8]) -> String {
    format!("SHA256:{}", util::b64_nopad(&util::sha256(blob)))
}

/// Describes a SubjectPublicKeyInfo element.
pub fn describe_spki(spki: &Tlv) -> Option<KeyInfo> {
    let mut r = spki.children();
    let alg = r.next()?;
    let key = r.next()?;
    let mut a = alg.children();
    let alg_oid = der::oid(&a.next()?)?;
    let params = a.next();
    let key_bits = der::bits(&key)?;
    let mut info = KeyInfo {
        algorithm: oid::label(&alg_oid),
        spki_sha256: util::colon_hex(&util::sha256(spki.raw), true),
        ..KeyInfo::default()
    };
    match alg_oid.as_str() {
        "1.2.840.113549.1.1.1" | "1.2.840.113549.1.1.10" => {
            let seq = der::top(key_bits)?;
            let mut k = seq.children();
            let n = k.next()?;
            let e = k.next()?;
            info.size = Some(format!("{} bits", der::bit_len(n.content)));
            let mut blob = Vec::new();
            ssh_string(&mut blob, b"ssh-rsa");
            ssh_mpint(&mut blob, e.content);
            ssh_mpint(&mut blob, n.content);
            info.ssh_sha256 = Some(ssh_fp(&blob));
        }
        "1.2.840.10045.2.1" => {
            let curve = params.as_ref().and_then(der::oid).map(|o| oid::label(&o));
            if let Some(c) = &curve {
                info.size = Some(match oid::curve_bits(c) {
                    Some(b) => format!("{c} ({b} bits)"),
                    None => c.clone(),
                });
                let ssh = match c.as_str() {
                    "P-256" => Some("nistp256"),
                    "P-384" => Some("nistp384"),
                    "P-521" => Some("nistp521"),
                    _ => None,
                };
                if let Some(s) = ssh {
                    let mut blob = Vec::new();
                    ssh_string(&mut blob, format!("ecdsa-sha2-{s}").as_bytes());
                    ssh_string(&mut blob, s.as_bytes());
                    ssh_string(&mut blob, key_bits);
                    info.ssh_sha256 = Some(ssh_fp(&blob));
                }
            }
        }
        "1.2.840.10040.4.1" => {
            if let Some(p) = params.as_ref().and_then(|p| p.children().next()) {
                info.size = Some(format!("{} bits", der::bit_len(p.content)));
            }
        }
        "1.3.101.112" => {
            info.size = Some("256 bits".into());
            let mut blob = Vec::new();
            ssh_string(&mut blob, b"ssh-ed25519");
            ssh_string(&mut blob, key_bits);
            info.ssh_sha256 = Some(ssh_fp(&blob));
        }
        "1.3.101.110" => info.size = Some("256 bits".into()),
        "1.3.101.111" | "1.3.101.113" => info.size = Some("448 bits".into()),
        _ => {}
    }
    Some(info)
}

fn algid_name(t: &Tlv) -> String {
    t.children()
        .next()
        .and_then(|o| der::oid(&o))
        .map_or_else(|| "unknown".into(), |o| oid::label(&o))
}

/// Serial number as uppercase hex.
pub fn serial_hex(content: &[u8]) -> String {
    let mut c = content;
    while c.len() > 1 && c[0] == 0 {
        c = &c[1..];
    }
    util::hex(c).to_uppercase()
}

/// A parsed certificate.
pub struct Cert<'a> {
    pub raw: &'a [u8],
    pub version: u64,
    pub serial: String,
    pub sig_alg: String,
    pub issuer: Name,
    pub subject: Name,
    pub issuer_raw: &'a [u8],
    pub subject_raw: &'a [u8],
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    pub key: Option<KeyInfo>,
    pub extensions: Vec<Tlv<'a>>,
}

/// Parses a DER certificate at the start of `data`.
pub fn parse_cert(data: &[u8]) -> Option<Cert<'_>> {
    let top = der::top(data)?;
    if top.id != der::SEQUENCE {
        return None;
    }
    let mut r = top.children();
    let tbs = r.next()?;
    let sig_alg = r.next()?;
    let sig = r.next()?;
    if tbs.id != der::SEQUENCE || sig_alg.id != der::SEQUENCE || sig.id != der::BIT_STRING {
        return None;
    }
    let mut t = tbs.children().peekable();
    let mut version = 1;
    if t.peek().is_some_and(|x| x.is_context(0)) {
        let v = t.next()?;
        version = v
            .children()
            .next()
            .and_then(|i| der::small_int(&i))
            .unwrap_or(0)
            + 1;
    }
    let serial = t.next()?;
    if serial.id != der::INTEGER {
        return None;
    }
    let _sig_inner = t.next()?;
    let issuer = t.next()?;
    let validity = t.next()?;
    let subject = t.next()?;
    let spki = t.next()?;
    if issuer.id != der::SEQUENCE || validity.id != der::SEQUENCE || subject.id != der::SEQUENCE {
        return None;
    }
    let mut v = validity.children();
    let not_before = v.next().and_then(|x| der::time(&x));
    let not_after = v.next().and_then(|x| der::time(&x));
    let mut extensions = Vec::new();
    for rest in t {
        if rest.is_context(3)
            && let Some(seq) = rest.children().next()
        {
            extensions = seq.items();
        }
    }
    Some(Cert {
        raw: top.raw,
        version,
        serial: serial_hex(serial.content),
        sig_alg: algid_name(&sig_alg),
        issuer: parse_name(&issuer),
        subject: parse_name(&subject),
        issuer_raw: issuer.content,
        subject_raw: subject.content,
        not_before,
        not_after,
        key: describe_spki(&spki),
        extensions,
    })
}

impl Cert<'_> {
    pub fn title(&self) -> String {
        self.subject
            .cn
            .clone()
            .or_else(|| (!self.subject.text.is_empty()).then(|| self.subject.text.clone()))
            .unwrap_or_else(|| "Certificate".into())
    }

    pub fn self_signed(&self) -> bool {
        self.issuer_raw == self.subject_raw
    }

    pub fn sha1(&self) -> String {
        util::colon_hex(&util::sha1(self.raw), true)
    }

    pub fn sha256(&self) -> String {
        util::colon_hex(&util::sha256(self.raw), true)
    }

    /// Identity: subject and issuer, so a renewed certificate pairs with
    /// the one it replaces.
    pub fn key(&self) -> String {
        format!("cert:{}|{}", self.subject.text, self.issuer.text)
    }

    /// The full item.
    pub fn item(&self) -> SigningItem {
        let mut item = SigningItem::new(SigningItemKind::Certificate, self.key(), self.title());
        item.not_before = self.not_before;
        item.not_after = self.not_after;
        item.field("Subject", self.subject.text.clone());
        name_parts(&mut item, "Subject", &self.subject);
        item.field("Issuer", self.issuer.text.clone());
        name_parts(&mut item, "Issuer", &self.issuer);
        item.field("Serial number", self.serial.clone());
        item.field("Version", self.version.to_string());
        validity_fields(&mut item, self.not_before, self.not_after);
        if let Some(k) = &self.key {
            k.add_fields(&mut item);
        }
        item.field("Signature algorithm", self.sig_alg.clone());
        item.field("Self-signed", if self.self_signed() { "Yes" } else { "No" });
        item.field("SHA-256 fingerprint", self.sha256());
        item.field("SHA-1 fingerprint", self.sha1());
        extension_fields(&mut item, &self.extensions);
        item
    }

    /// The short form used inside profiles and keystores.
    pub fn summary_item(&self) -> SigningItem {
        let mut item = SigningItem::new(SigningItemKind::Certificate, self.key(), self.title());
        item.not_before = self.not_before;
        item.not_after = self.not_after;
        item.field("Subject", self.subject.text.clone());
        item.field("Issuer", self.issuer.text.clone());
        item.field("Serial number", self.serial.clone());
        validity_fields(&mut item, self.not_before, self.not_after);
        if let Some(k) = &self.key {
            item.field("Public key", k.summary());
        }
        item.field("SHA-1 fingerprint", self.sha1());
        item
    }
}

/// A certificate as an item, or a placeholder when it does not parse.
pub fn cert_item(der_bytes: &[u8], summary: bool) -> SigningItem {
    match parse_cert(der_bytes) {
        Some(c) if summary => c.summary_item(),
        Some(c) => c.item(),
        None => {
            let fp = util::colon_hex(&util::sha1(der_bytes), true);
            let mut item = SigningItem::new(
                SigningItemKind::Certificate,
                format!("cert:{fp}"),
                "Unreadable certificate",
            );
            item.field("SHA-1 fingerprint", fp);
            item.field("Size", format!("{} bytes", der_bytes.len()));
            item
        }
    }
}

fn name_parts(item: &mut SigningItem, prefix: &str, name: &Name) {
    if let Some(cn) = &name.cn {
        item.field(format!("{prefix} CN"), cn.clone());
    }
    if !name.o.is_empty() {
        item.field(format!("{prefix} O"), name.o.join(", "));
    }
    if !name.ou.is_empty() {
        item.field(format!("{prefix} OU"), name.ou.join(", "));
    }
}

/// Not before / not after fields with unix seconds kept on the item.
pub fn validity_fields(item: &mut SigningItem, nb: Option<i64>, na: Option<i64>) {
    if let Some(nb) = nb {
        item.field("Not before", util::utc(nb));
    }
    if let Some(na) = na {
        item.field("Not after", util::utc(na));
    }
    if let (Some(nb), Some(na)) = (nb, na) {
        item.field("Validity period", util::duration(na - nb));
    }
}

fn general_name(t: &Tlv) -> Option<(&'static str, String)> {
    if t.id & 0xC0 != 0x80 {
        return None;
    }
    let text = || String::from_utf8_lossy(t.content).into_owned();
    Some(match t.num {
        1 => ("Email", text()),
        2 => ("DNS", text()),
        6 => ("URI", text()),
        7 => ("IP", ip(t.content)),
        4 => (
            "Directory",
            t.children()
                .next()
                .map(|n| parse_name(&n).text)
                .unwrap_or_default(),
        ),
        0 => (
            "Other name",
            t.children()
                .next()
                .and_then(|o| der::oid(&o))
                .unwrap_or_default(),
        ),
        8 => (
            "Registered ID",
            der::oid_string(t.content).unwrap_or_default(),
        ),
        _ => return None,
    })
}

fn ip(b: &[u8]) -> String {
    match b.len() {
        4 => format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]),
        16 => {
            let mut a = [0u8; 16];
            a.copy_from_slice(b);
            std::net::Ipv6Addr::from(a).to_string()
        }
        8 => format!(
            "{}.{}.{}.{}/{}.{}.{}.{}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]
        ),
        _ => util::hex(b),
    }
}

/// URIs anywhere below an element ([6] general names), depth bounded.
fn uris(t: &Tlv, depth: usize, out: &mut Vec<String>) {
    if depth > 8 {
        return;
    }
    for c in t.children() {
        if c.id == 0x86 {
            out.push(String::from_utf8_lossy(c.content).into_owned());
        } else if c.constructed() {
            uris(&c, depth + 1, out);
        }
    }
}

const KEY_USAGE: [&str; 9] = [
    "Digital signature",
    "Non-repudiation",
    "Key encipherment",
    "Data encipherment",
    "Key agreement",
    "Certificate signing",
    "CRL signing",
    "Encipher only",
    "Decipher only",
];

/// Fields for each extension of a certificate or request.
pub fn extension_fields(item: &mut SigningItem, exts: &[Tlv]) {
    let mut apple = Vec::new();
    for ext in exts {
        let mut r = ext.children().peekable();
        let Some(dotted) = r.next().and_then(|o| der::oid(&o)) else {
            continue;
        };
        let mut critical = false;
        if r.peek().is_some_and(|t| t.id == der::BOOLEAN) {
            critical = r
                .next()
                .is_some_and(|b| b.content.first().is_some_and(|&v| v != 0));
        }
        let Some(value) = r.next().filter(|v| v.id == der::OCTET_STRING) else {
            continue;
        };
        let v = der::top(value.content);
        let crit = if critical { " (critical)" } else { "" };
        match (dotted.as_str(), v) {
            ("2.5.29.17", Some(v)) | ("2.5.29.18", Some(v)) => {
                let prefix = if dotted == "2.5.29.17" {
                    "SAN"
                } else {
                    "Issuer alt name"
                };
                let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
                for n in v.children() {
                    if let Some((kind, val)) = general_name(&n) {
                        match groups.iter_mut().find(|g| g.0 == kind) {
                            Some(g) => g.1.push(val),
                            None => groups.push((kind, vec![val])),
                        }
                    }
                }
                for (kind, vals) in groups {
                    item.field(format!("{prefix} {kind}{crit}"), vals.join(", "));
                }
            }
            ("2.5.29.15", Some(v)) => {
                let bits = der::bits(&v).unwrap_or(&[]);
                let names: Vec<&str> = KEY_USAGE
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| bits.get(i / 8).is_some_and(|b| b & (0x80 >> (i % 8)) != 0))
                    .map(|(_, n)| *n)
                    .collect();
                item.field(format!("Key usage{crit}"), names.join(", "));
            }
            ("2.5.29.37", Some(v)) => {
                let names: Vec<String> = v
                    .children()
                    .filter_map(|o| der::oid(&o))
                    .map(|o| oid::label(&o))
                    .collect();
                item.field(format!("Extended key usage{crit}"), names.join(", "));
            }
            ("2.5.29.19", Some(v)) => {
                let mut ca = false;
                let mut path = None;
                for c in v.children() {
                    if c.id == der::BOOLEAN {
                        ca = c.content.first().is_some_and(|&b| b != 0);
                    } else if c.id == der::INTEGER {
                        path = der::small_int(&c);
                    }
                }
                let mut s = if ca {
                    "CA".to_string()
                } else {
                    "Not a CA".to_string()
                };
                if let Some(p) = path {
                    s.push_str(&format!(", path length {p}"));
                }
                item.field(format!("Basic constraints{crit}"), s);
            }
            ("2.5.29.14", Some(v)) => {
                item.field("Subject key ID", util::colon_hex(v.content, true));
            }
            ("2.5.29.35", Some(v)) => {
                if let Some(k) = v.children().find(|c| c.is_context(0)) {
                    item.field("Authority key ID", util::colon_hex(k.content, true));
                }
            }
            ("2.5.29.32", Some(v)) => {
                let names: Vec<String> = v
                    .children()
                    .filter_map(|p| p.children().next().and_then(|o| der::oid(&o)))
                    .map(|o| match oid::name(&o) {
                        Some(n) => format!("{o} ({n})"),
                        None => o,
                    })
                    .collect();
                item.field(format!("Certificate policies{crit}"), names.join(", "));
            }
            ("2.5.29.31", Some(v)) => {
                let mut out = Vec::new();
                uris(&v, 0, &mut out);
                item.field("CRL distribution points", out.join(", "));
            }
            ("1.3.6.1.5.5.7.1.1", Some(v)) => {
                let mut ocsp = Vec::new();
                let mut issuers = Vec::new();
                for ad in v.children() {
                    let mut a = ad.children();
                    let (Some(m), Some(loc)) = (a.next().and_then(|o| der::oid(&o)), a.next())
                    else {
                        continue;
                    };
                    let Some((_, val)) = general_name(&loc) else {
                        continue;
                    };
                    match m.as_str() {
                        "1.3.6.1.5.5.7.48.1" => ocsp.push(val),
                        "1.3.6.1.5.5.7.48.2" => issuers.push(val),
                        _ => {}
                    }
                }
                if !ocsp.is_empty() {
                    item.field("OCSP", ocsp.join(", "));
                }
                if !issuers.is_empty() {
                    item.field("CA issuers", issuers.join(", "));
                }
            }
            (o, _) if o.starts_with("1.2.840.113635.100.6.") => {
                apple.push(oid::name(o).map_or_else(|| o.to_string(), str::to_string));
            }
            (o, _) => {
                let label = match oid::name(o) {
                    Some(n) => format!("Extension {n}"),
                    None => format!("Extension {o}"),
                };
                let state = if critical {
                    "present, critical"
                } else {
                    "present"
                };
                item.field(label, format!("{state}, {} bytes", value.content.len()));
            }
        }
    }
    if !apple.is_empty() {
        item.field("Apple certificate type", apple.join(", "));
    }
}

/// A PKCS#10 certificate signing request.
pub fn csr_item(data: &[u8]) -> Option<SigningItem> {
    let top = der::top(data)?;
    let mut r = top.children();
    let info = r.next()?;
    let sig_alg = r.next()?;
    r.next().filter(|s| s.id == der::BIT_STRING)?;
    let mut i = info.children();
    let version = i.next().filter(|v| v.id == der::INTEGER)?;
    let subject = i.next().filter(|s| s.id == der::SEQUENCE)?;
    let spki = i.next().filter(|s| s.id == der::SEQUENCE)?;
    let key = describe_spki(&spki)?;
    let name = parse_name(&subject);
    let title = name.cn.clone().unwrap_or_else(|| name.text.clone());
    let mut item = SigningItem::new(
        SigningItemKind::CertificateRequest,
        format!("csr:{}", name.text),
        if title.is_empty() {
            "Certificate signing request".into()
        } else {
            title
        },
    );
    item.field("Subject", name.text.clone());
    name_parts(&mut item, "Subject", &name);
    item.field(
        "Version",
        (der::small_int(&version).unwrap_or(0) + 1).to_string(),
    );
    key.add_fields(&mut item);
    item.field("Signature algorithm", algid_name(&sig_alg));
    if let Some(attrs) = i.find(|a| a.is_context(0)) {
        for attr in attrs.children() {
            let mut a = attr.children();
            let Some(o) = a.next().and_then(|o| der::oid(&o)) else {
                continue;
            };
            let values = a.next();
            match o.as_str() {
                "1.2.840.113549.1.9.14" => {
                    if let Some(exts) = values.and_then(|v| v.children().next()) {
                        extension_fields(&mut item, &exts.items());
                    }
                }
                // The challenge password is a secret: only say it is there.
                "1.2.840.113549.1.9.7" => item.field("Challenge password", "present"),
                other => {
                    let v = values
                        .and_then(|v| v.children().next())
                        .and_then(|s| der::string(&s))
                        .unwrap_or_else(|| "present".into());
                    item.field(format!("Attribute {}", oid::label(other)), v);
                }
            }
        }
    }
    item.field("SHA-256", util::colon_hex(&util::sha256(top.raw), true));
    Some(item)
}

/// An X.509 CRL.
pub fn crl_item(data: &[u8]) -> Option<SigningItem> {
    let top = der::top(data)?;
    let mut r = top.children();
    let tbs = r.next()?;
    let sig_alg = r.next()?;
    r.next().filter(|s| s.id == der::BIT_STRING)?;
    let mut t = tbs.children().peekable();
    let mut version = 1;
    if t.peek().is_some_and(|v| v.id == der::INTEGER) {
        version = t.next().and_then(|v| der::small_int(&v)).unwrap_or(0) + 1;
    }
    t.next().filter(|a| a.id == der::SEQUENCE)?;
    let issuer = t.next().filter(|s| s.id == der::SEQUENCE)?;
    let this = t.next().and_then(|x| der::time(&x))?;
    let mut next = None;
    if t.peek().is_some_and(|x| x.id == 0x17 || x.id == 0x18) {
        next = t.next().and_then(|x| der::time(&x));
    }
    let name = parse_name(&issuer);
    let mut item = SigningItem::new(
        SigningItemKind::RevocationList,
        format!("crl:{}", name.text),
        name.cn.clone().unwrap_or_else(|| name.text.clone()),
    );
    item.field("Issuer", name.text.clone());
    item.field("Version", version.to_string());
    item.field("This update", util::utc(this));
    item.not_before = Some(this);
    if let Some(n) = next {
        item.field("Next update", util::utc(n));
        item.not_after = Some(n);
    }
    item.field("Signature algorithm", algid_name(&sig_alg));
    let mut count = 0usize;
    for rest in t {
        if rest.id == der::SEQUENCE {
            for entry in rest.children() {
                count += 1;
                if item.children.len() >= MAX_REVOKED {
                    continue;
                }
                let mut e = entry.children();
                let Some(serial) = e.next() else { continue };
                let serial = serial_hex(serial.content);
                let mut child = SigningItem::new(
                    SigningItemKind::RevokedCertificate,
                    format!("revoked:{serial}"),
                    serial.clone(),
                );
                if let Some(d) = e.next().and_then(|x| der::time(&x)) {
                    child.field("Revoked", util::utc(d));
                }
                item.children.push(child);
            }
        } else if rest.is_context(0)
            && let Some(seq) = rest.children().next()
        {
            extension_fields(&mut item, &seq.items());
        }
    }
    item.field("Revoked certificates", count.to_string());
    Some(item)
}

/// A SubjectPublicKeyInfo (PEM `PUBLIC KEY`).
pub fn public_key_item(spki_der: &[u8]) -> Option<SigningItem> {
    let spki = der::top(spki_der)?;
    let info = describe_spki(&spki)?;
    let mut item = SigningItem::new(
        SigningItemKind::PublicKey,
        format!("key:{}", info.spki_sha256),
        format!("{} public key", info.summary()),
    );
    info.add_fields(&mut item);
    Some(item)
}

/// A PKCS#1 `RSA PUBLIC KEY`.
pub fn rsa_public_item(pkcs1: &[u8]) -> Option<SigningItem> {
    let seq = der::top(pkcs1)?;
    let mut r = seq.children();
    let n = r.next().filter(|t| t.id == der::INTEGER)?;
    let e = r.next().filter(|t| t.id == der::INTEGER)?;
    public_key_item(&rsa_spki(n.content, e.content))
}

fn algid(oid_dotted: &str, params: Option<&[u8]>) -> Vec<u8> {
    let mut c = der::write(der::OID, &der::encode_oid(oid_dotted));
    match params {
        Some(p) => c.extend_from_slice(p),
        None => c.extend_from_slice(&[der::NULL, 0]),
    }
    der::write(der::SEQUENCE, &c)
}

fn spki(alg: Vec<u8>, key: &[u8]) -> Vec<u8> {
    let mut bits = vec![0u8];
    bits.extend_from_slice(key);
    let mut c = alg;
    c.extend(der::write(der::BIT_STRING, &bits));
    der::write(der::SEQUENCE, &c)
}

fn rsa_spki(n: &[u8], e: &[u8]) -> Vec<u8> {
    let mut ne = der::write_uint(n);
    ne.extend(der::write_uint(e));
    spki(
        algid("1.2.840.113549.1.1.1", None),
        &der::write(der::SEQUENCE, &ne),
    )
}

fn private_item(algorithm: String, info: Option<KeyInfo>, protection: &str) -> SigningItem {
    let summary = info.as_ref().map_or(algorithm.clone(), KeyInfo::summary);
    let key = match &info {
        Some(i) => format!("private:{}", i.spki_sha256),
        None => format!("private:{algorithm}"),
    };
    let mut item = SigningItem::new(
        SigningItemKind::PrivateKey,
        key,
        format!("{summary} private key"),
    );
    item.field("Private key", "present (not shown)");
    item.field("Algorithm", algorithm);
    item.field("Protection", protection);
    if let Some(i) = info {
        i.add_fields(&mut item);
    }
    item
}

/// Public half of a PKCS#1 RSA private key: only `n` and `e` are read.
fn rsa_private_public(seq: &Tlv) -> Option<KeyInfo> {
    let mut r = seq.children();
    r.next().filter(|v| v.id == der::INTEGER)?;
    let n = r.next().filter(|t| t.id == der::INTEGER)?;
    let e = r.next().filter(|t| t.id == der::INTEGER)?;
    describe_spki(&der::top(&rsa_spki(n.content, e.content))?)
}

/// Public half of an SEC 1 EC private key: the private octets are skipped
/// and only the curve and the optional public point are read.
fn ec_private_public(seq: &Tlv, curve: Option<String>) -> (Option<String>, Option<KeyInfo>) {
    let mut curve = curve;
    let mut point = None;
    for c in seq.children() {
        if c.is_context(0) {
            if let Some(o) = c.children().next().and_then(|o| der::oid(&o)) {
                curve = Some(o);
            }
        } else if c.is_context(1) {
            point = c
                .children()
                .next()
                .and_then(|b| der::bits(&b).map(<[u8]>::to_vec));
        }
    }
    let info = match (&curve, point) {
        (Some(cv), Some(p)) => {
            let params = der::write(der::OID, &der::encode_oid(cv));
            der::top(&spki(algid("1.2.840.10045.2.1", Some(&params)), &p))
                .and_then(|t| describe_spki(&t))
        }
        _ => None,
    };
    (curve.map(|c| oid::label(&c)), info)
}

/// A PEM private key block, read only for its type and public half.
/// `encrypted_header` is the DEK-Info cipher of a traditional encrypted
/// PEM key.
pub fn private_key_item(
    label: &str,
    der_bytes: &[u8],
    encrypted_header: Option<&str>,
) -> SigningItem {
    if let Some(cipher) = encrypted_header {
        let alg = label.trim_end_matches(" PRIVATE KEY").to_string();
        return private_item(alg, None, &format!("encrypted ({cipher})"));
    }
    let top = der::top(der_bytes);
    match (label, top) {
        ("RSA PRIVATE KEY", Some(seq)) => {
            private_item("RSA".into(), rsa_private_public(&seq), "unencrypted")
        }
        ("EC PRIVATE KEY", Some(seq)) => {
            let (curve, info) = ec_private_public(&seq, None);
            let alg = curve.map_or("EC".into(), |c| format!("EC {c}"));
            private_item(alg, info, "unencrypted")
        }
        ("DSA PRIVATE KEY", Some(seq)) => {
            let mut r = seq.children();
            let p = r.nth(1);
            let info = p.map(|p| KeyInfo {
                algorithm: "DSA".into(),
                size: Some(format!("{} bits", der::bit_len(p.content))),
                ..KeyInfo::default()
            });
            let alg = info.as_ref().map_or("DSA".into(), KeyInfo::summary);
            private_item(alg, None, "unencrypted")
        }
        ("ENCRYPTED PRIVATE KEY", Some(seq)) => encrypted_pkcs8_item(&seq),
        ("PRIVATE KEY", Some(seq)) => pkcs8_item(&seq),
        _ => private_item(
            label.trim_end_matches(" PRIVATE KEY").to_string(),
            None,
            "unknown",
        ),
    }
}

/// An unencrypted PKCS#8 PrivateKeyInfo: algorithm and public half only.
pub fn pkcs8_item(seq: &Tlv) -> SigningItem {
    let mut r = seq.children();
    let _version = r.next();
    let Some(alg) = r.next() else {
        return private_item("unknown".into(), None, "unencrypted");
    };
    let mut a = alg.children();
    let alg_oid = a.next().and_then(|o| der::oid(&o)).unwrap_or_default();
    let params = a.next();
    let inner = r.next().filter(|o| o.id == der::OCTET_STRING);
    let public = r.find(|t| t.is_context(1));
    let name = oid::label(&alg_oid);
    let info = match alg_oid.as_str() {
        "1.2.840.113549.1.1.1" | "1.2.840.113549.1.1.10" => inner
            .and_then(|o| der::top(o.content))
            .and_then(|s| rsa_private_public(&s)),
        "1.2.840.10045.2.1" => {
            let curve = params.as_ref().and_then(der::oid);
            inner
                .and_then(|o| der::top(o.content))
                .and_then(|s| ec_private_public(&s, curve).1)
        }
        "1.3.101.112" | "1.3.101.110" | "1.3.101.111" | "1.3.101.113" => public
            .as_ref()
            .and_then(|p| {
                // [1] IMPLICIT BIT STRING: content is the unused-bits octet plus key.
                p.content.get(1..)
            })
            .and_then(|k| {
                der::top(&spki(algid(&alg_oid, Some(&[])), k)).and_then(|t| describe_spki(&t))
            }),
        _ => None,
    };
    private_item(name, info, "unencrypted")
}

/// A PKCS#8 EncryptedPrivateKeyInfo: algorithm and iteration count only.
pub fn encrypted_pkcs8_item(seq: &Tlv) -> SigningItem {
    let (scheme, iterations) = seq
        .children()
        .next()
        .map(|a| crate::pkcs12::describe_pbe(&a))
        .unwrap_or_default();
    let mut protection = format!("encrypted ({scheme})");
    if let Some(i) = iterations {
        protection.push_str(&format!(", {i} iterations"));
    }
    private_item("unknown (encrypted)".into(), None, &protection)
}

/// Whether a DER blob parses as one of the structures above, for binary
/// detection: returns the items and the file kind.
pub fn read_der(data: &[u8]) -> Option<(corvene_models::signing::SigningKind, Vec<SigningItem>)> {
    use corvene_models::signing::SigningKind as K;
    if let Some(c) = parse_cert(data) {
        return Some((K::Certificate, vec![c.item()]));
    }
    if let Some(i) = csr_item(data) {
        return Some((K::CertificateRequest, vec![i]));
    }
    if let Some(i) = crl_item(data) {
        return Some((K::RevocationList, vec![i]));
    }
    let top = der::top(data)?;
    let kids: Vec<Tlv> = Reader::new(top.content).take(4).collect();
    match kids.as_slice() {
        [a, b] if a.id == der::SEQUENCE && b.id == der::BIT_STRING => {
            public_key_item(top.raw).map(|i| (K::PublicKey, vec![i]))
        }
        [a, b] if a.id == der::SEQUENCE && b.id == der::OCTET_STRING => {
            Some((K::PrivateKey, vec![encrypted_pkcs8_item(&top)]))
        }
        [a, b, c, ..]
            if a.id == der::INTEGER && b.id == der::SEQUENCE && c.id == der::OCTET_STRING =>
        {
            Some((K::PrivateKey, vec![pkcs8_item(&top)]))
        }
        [a, b, c, d]
            if a.id == der::INTEGER
                && b.id == der::INTEGER
                && c.id == der::INTEGER
                && d.id == der::INTEGER =>
        {
            let n = top.children().count();
            if n == 9 {
                Some((
                    K::PrivateKey,
                    vec![private_key_item("RSA PRIVATE KEY", top.raw, None)],
                ))
            } else if n == 6 {
                Some((
                    K::PrivateKey,
                    vec![private_key_item("DSA PRIVATE KEY", top.raw, None)],
                ))
            } else {
                None
            }
        }
        [a, b] if a.id == der::INTEGER && b.id == der::INTEGER => {
            rsa_public_item(top.raw).map(|i| (K::PublicKey, vec![i]))
        }
        [a, b, ..]
            if a.id == der::INTEGER
                && b.id == der::OCTET_STRING
                && der::small_int(a) == Some(1) =>
        {
            Some((
                K::PrivateKey,
                vec![private_key_item("EC PRIVATE KEY", top.raw, None)],
            ))
        }
        _ => None,
    }
}
