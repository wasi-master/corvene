//! Corvene `1341-signing-material-diff`: certificates, PKCS#12 bundles,
//! provisioning profiles, Java keystores, OpenPGP and SSH keys read as
//! their public fields. Private keys are never decoded or shown.
//!
//! GitHub Desktop shows these files as binary or text diffs
//! (`app/src/ui/diff/index.tsx`). Private key material is only ever
//! reported as present: its type, size and the fingerprint of the public
//! half when that half is stored in clear. PKCS#12 key bags and protected
//! keystore keys are never decrypted, and no password is kept.

mod crypto;
mod der;
mod jks;
mod oid;
mod pgp;
mod pkcs12;
mod pkcs7;
mod plist;
mod profile;
mod ssh;
mod util;
mod x509;

use corvene_models::signing::{SigningFile, SigningItem, SigningItemKind, SigningKind};

const EXTENSIONS: &[&str] = &[
    "pem",
    "crt",
    "cer",
    "der",
    "ca-bundle",
    "csr",
    "p7b",
    "p7c",
    "crl",
    "p12",
    "pfx",
    "mobileprovision",
    "provisionprofile",
    "jks",
    "keystore",
    "jceks",
    "bks",
    "asc",
    "gpg",
    "pgp",
    "pub",
    "key",
];

const NAMES: &[&str] = &[
    "authorized_keys",
    "authorized_keys2",
    "known_hosts",
    "known_hosts2",
    "ssh_known_hosts",
    "ssh_known_hosts2",
    "allowed_signers",
    "embedded.provisionprofile",
];

fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
}

/// Whether `path` names signing material this crate reads (by name).
pub fn is_signing_file(path: &str) -> bool {
    let name = base_name(path);
    if NAMES.contains(&name.as_str()) {
        return true;
    }
    if name.starts_with("id_") && !name.contains('.') {
        return true;
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && EXTENSIONS.contains(&ext),
        None => false,
    }
}

/// The public fields of the file at `path`. A password-protected file
/// read with no or a wrong `password` comes back with
/// [`SigningFile::needs_password`] set and its bytes in `raw`. `None` when
/// it is not readable as signing material at all.
pub fn read(path: &str, bytes: &[u8], password: Option<&str>) -> Option<SigningFile> {
    let name = base_name(path);
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_string())
        .unwrap_or_default();
    if bytes.starts_with(&[0xFE, 0xED, 0xFE, 0xED]) || bytes.starts_with(&[0xCE, 0xCE, 0xCE, 0xCE])
    {
        return jks::read(bytes);
    }
    if matches!(ext.as_str(), "bks" | "uber") {
        return Some(SigningFile {
            kind: SigningKind::Unsupported,
            notes: vec!["Bouncy Castle keystores (BKS, UBER) are not supported".into()],
            ..SigningFile::default()
        });
    }
    if let Some(text) = util::text(bytes) {
        let found = read_text(&name, text);
        if found.is_some() {
            return found;
        }
    }
    read_binary(bytes, password)
}

fn read_text(name: &str, text: &str) -> Option<SigningFile> {
    if text.contains("-----BEGIN PGP ")
        && let Some(f) = pgp::read_armored(text)
    {
        return Some(f);
    }
    if text.contains("-----BEGIN ")
        && let Some(f) = read_pem(text)
    {
        return Some(f);
    }
    if text.contains("---- BEGIN SSH2 PUBLIC KEY ----") {
        let items: Vec<SigningItem> = text
            .split("---- BEGIN SSH2 PUBLIC KEY ----")
            .skip(1)
            .filter_map(|b| b.split("---- END SSH2 PUBLIC KEY ----").next())
            .filter_map(ssh::rfc4716)
            .collect();
        if !items.is_empty() {
            return Some(SigningFile {
                kind: SigningKind::SshPublicKey,
                items,
                ..SigningFile::default()
            });
        }
    }
    ssh::read_lines(name, text)
}

fn read_binary(bytes: &[u8], password: Option<&str>) -> Option<SigningFile> {
    match bytes.first()? {
        0x30 => {
            if let Some(f) = pkcs12::read(bytes, password) {
                return Some(f);
            }
            if let Some(f) = profile::read(bytes) {
                return Some(f);
            }
            if let Some(items) = pkcs7_items(bytes) {
                return Some(SigningFile {
                    kind: SigningKind::Pkcs7,
                    items,
                    ..SigningFile::default()
                });
            }
            let (kind, items) = x509::read_der(bytes)?;
            let mut f = SigningFile {
                kind,
                items,
                ..SigningFile::default()
            };
            if kind == SigningKind::PrivateKey {
                f.notes
                    .push("Private key material is never decoded or shown".into());
            }
            Some(f)
        }
        b if b & 0x80 != 0 => pgp::read_binary(bytes),
        _ => None,
    }
}

fn pkcs7_items(bytes: &[u8]) -> Option<Vec<SigningItem>> {
    let sd = pkcs7::parse(bytes)?;
    let mut items: Vec<SigningItem> = sd.certs.iter().map(|c| x509::cert_item(c, false)).collect();
    if let Some(s) = pkcs7::signer_cert(&sd) {
        let mut i = SigningItem::new(
            SigningItemKind::Signer,
            format!("signer:{}", s.key()),
            s.title(),
        );
        i.field("Signer", s.subject.text.clone());
        items.push(i);
    }
    if sd.crls > 0 {
        let mut i = SigningItem::new(SigningItemKind::Container, "crls", "Revocation lists");
        i.field("CRLs", sd.crls.to_string());
        items.push(i);
    }
    Some(items)
}

struct PemBlock<'a> {
    label: &'a str,
    headers: Vec<(&'a str, &'a str)>,
    data: Option<Vec<u8>>,
}

fn pem_blocks(text: &str) -> Vec<PemBlock<'_>> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(label) = line
            .trim()
            .strip_prefix("-----BEGIN ")
            .and_then(|l| l.strip_suffix("-----"))
        else {
            continue;
        };
        let mut headers = Vec::new();
        let mut b64 = String::new();
        for l in lines.by_ref() {
            let l = l.trim();
            if l.starts_with("-----END ") {
                break;
            }
            match l.split_once(':') {
                Some((k, v)) if b64.is_empty() => headers.push((k.trim(), v.trim())),
                _ => b64.push_str(l),
            }
        }
        out.push(PemBlock {
            label,
            headers,
            data: util::b64(&b64),
        });
    }
    out
}

fn read_pem(text: &str) -> Option<SigningFile> {
    let mut file = SigningFile::default();
    let mut kinds: Vec<SigningKind> = Vec::new();
    let mut private = false;
    for block in pem_blocks(text) {
        let Some(data) = &block.data else {
            file.notes
                .push(format!("{} block could not be decoded", block.label));
            continue;
        };
        let (kind, items) = match block.label {
            "CERTIFICATE" | "X509 CERTIFICATE" | "TRUSTED CERTIFICATE" => {
                (SigningKind::Certificate, vec![x509::cert_item(data, false)])
            }
            "CERTIFICATE REQUEST" | "NEW CERTIFICATE REQUEST" => (
                SigningKind::CertificateRequest,
                x509::csr_item(data).into_iter().collect(),
            ),
            "X509 CRL" => (
                SigningKind::RevocationList,
                x509::crl_item(data).into_iter().collect(),
            ),
            "PKCS7" | "CMS" => (SigningKind::Pkcs7, pkcs7_items(data).unwrap_or_default()),
            "PUBLIC KEY" => (
                SigningKind::PublicKey,
                x509::public_key_item(data).into_iter().collect(),
            ),
            "RSA PUBLIC KEY" => (
                SigningKind::PublicKey,
                x509::rsa_public_item(data).into_iter().collect(),
            ),
            "OPENSSH PRIVATE KEY" => {
                private = true;
                (
                    SigningKind::SshPrivateKey,
                    ssh::openssh_private(data).unwrap_or_default(),
                )
            }
            l if l.ends_with("PRIVATE KEY") => {
                private = true;
                let dek = block
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("DEK-Info"))
                    .map(|(_, v)| v.split(',').next().unwrap_or(v).trim());
                (
                    SigningKind::PrivateKey,
                    vec![x509::private_key_item(l, data, dek)],
                )
            }
            other => {
                file.notes.push(format!("{other} block not shown"));
                continue;
            }
        };
        if items.is_empty() {
            file.notes
                .push(format!("{} block could not be read", block.label));
            continue;
        }
        kinds.push(kind);
        file.items.extend(items);
    }
    if file.items.is_empty() {
        return None;
    }
    let first = kinds[0];
    file.kind = if kinds.iter().all(|k| *k == first) {
        if first == SigningKind::Certificate && kinds.len() > 1 {
            SigningKind::CertificateBundle
        } else {
            first
        }
    } else {
        SigningKind::Mixed
    };
    if private {
        file.notes
            .push("Private key material is never decoded or shown".into());
    }
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_signing_file("a/b/cert.pem"));
        assert!(is_signing_file("Profiles/App.mobileprovision"));
        assert!(is_signing_file(".ssh/known_hosts"));
        assert!(is_signing_file("id_ed25519"));
        assert!(is_signing_file("id_ed25519.pub"));
        assert!(!is_signing_file("src/main.rs"));
        assert!(!is_signing_file(".pem"));
    }
}
