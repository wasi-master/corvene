//! Signing material readers over fixtures generated with openssl,
//! ssh-keygen and a hand-built OpenPGP key (`gpg --list-packets` agrees on
//! its key ID), plus provisioning profiles and Java keystores built here.

use base64::Engine;
use corvene_keys::{is_signing_file, read};
use corvene_models::signing::{
    PasswordState, RowStatus, SigningFile, SigningItem, SigningItemKind, SigningKind, align,
};

fn fx(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn open(name: &str, password: Option<&str>) -> SigningFile {
    read(name, &fx(name), password).unwrap_or_else(|| panic!("{name} not read"))
}

fn val<'a>(item: &'a SigningItem, label: &str) -> &'a str {
    item.value(label)
        .unwrap_or_else(|| panic!("no {label:?} in {:#?}", item.fields))
}

fn pem_der(name: &str) -> Vec<u8> {
    let text = String::from_utf8(fx(name)).unwrap();
    let b64: String = text
        .lines()
        .filter(|l| !l.starts_with("-----") && !l.contains(':'))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap()
}

/// No rendering of `secret` appears anywhere in the file's debug output.
fn assert_no_secret(file: &SigningFile, secret: &[u8]) {
    let dbg = format!("{file:?}");
    let hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    let colon: Vec<String> = secret.iter().map(|b| format!("{b:02X}")).collect();
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut needles = vec![
        hex.clone(),
        hex.to_uppercase(),
        colon.join(":"),
        colon.join(" "),
    ];
    for off in 0..3 {
        let s = b64.encode(&secret[off..]);
        needles.push(s[..s.len().saturating_sub(4)].to_string());
    }
    // The raw byte list Debug prints for `raw`.
    let list = secret
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    needles.push(list);
    for n in needles {
        assert!(!dbg.contains(&n), "secret material found: {n}");
    }
}

#[test]
fn names() {
    for p in [
        "a.pem",
        "x.p12",
        "x.mobileprovision",
        "known_hosts",
        "id_rsa",
        "k.asc",
        "store.jks",
    ] {
        assert!(is_signing_file(p), "{p}");
    }
    assert!(!is_signing_file("main.rs"));
}

#[test]
fn certificate_pem() {
    let f = open("leaf.pem", None);
    assert_eq!(f.kind, SigningKind::Certificate);
    let c = &f.items[0];
    assert_eq!(c.kind, SigningItemKind::Certificate);
    assert_eq!(c.title, "Apple Distribution: Acme Inc (ABCDE12345)");
    assert_eq!(
        val(c, "Subject"),
        "CN=Apple Distribution: Acme Inc (ABCDE12345), OU=Engineering, O=Acme Inc, C=US"
    );
    assert_eq!(val(c, "Subject O"), "Acme Inc");
    assert_eq!(val(c, "Subject OU"), "Engineering");
    assert_eq!(val(c, "Issuer"), "CN=Test Root CA, O=Test CA, C=US");
    assert_eq!(val(c, "Serial number"), "1234ABCD");
    assert_eq!(val(c, "Version"), "3");
    assert_eq!(val(c, "Not after"), "2029-01-14 14:05:57 UTC");
    assert_eq!(c.not_after, Some(1_863_093_957));
    assert!(c.not_before.is_some_and(|nb| nb < 1_863_093_957));
    assert_eq!(val(c, "Public key"), "RSA 2048 bits");
    assert_eq!(
        val(c, "Public key SHA-256"),
        "15:83:59:5D:A9:F7:8A:4A:D6:16:08:87:37:AB:37:38:E8:59:B8:25:98:2F:79:AD:07:B2:53:A6:50:95:07:C7"
    );
    assert_eq!(
        val(c, "SSH fingerprint"),
        "SHA256:QfqXAr6G56ImfWWra/16tNDOD7OEFzNMjXCz4D49gBo"
    );
    assert_eq!(val(c, "Signature algorithm"), "ecdsa-with-SHA256");
    assert_eq!(val(c, "Self-signed"), "No");
    assert_eq!(
        val(c, "SHA-256 fingerprint"),
        "AF:70:F1:2E:7F:82:41:2E:13:73:92:AA:29:CF:BE:74:F4:4D:1C:36:B0:63:AF:81:D9:AE:49:B3:A8:EB:46:CE"
    );
    assert_eq!(
        val(c, "SHA-1 fingerprint"),
        "8A:18:42:31:61:B0:1D:CC:FE:44:4D:96:1A:4A:60:65:A3:73:AD:61"
    );
    assert_eq!(val(c, "SAN DNS"), "example.com, www.example.com");
    assert_eq!(val(c, "SAN IP"), "192.0.2.1");
    assert_eq!(val(c, "SAN Email"), "dev@example.com");
    assert_eq!(val(c, "SAN URI"), "https://example.com/id");
    assert_eq!(
        val(c, "Key usage (critical)"),
        "Digital signature, Key encipherment"
    );
    assert_eq!(
        val(c, "Extended key usage"),
        "TLS server authentication, TLS client authentication, Code signing"
    );
    assert_eq!(val(c, "Basic constraints (critical)"), "Not a CA");
    assert_eq!(
        val(c, "Certificate policies"),
        "2.23.140.1.2.1 (Domain validated)"
    );
    assert_eq!(
        val(c, "CRL distribution points"),
        "http://crl.example.com/ca.crl"
    );
    assert_eq!(val(c, "OCSP"), "http://ocsp.example.com");
    assert_eq!(val(c, "CA issuers"), "http://example.com/ca.crt");
    assert_eq!(val(c, "Apple certificate type"), "Apple Distribution (iOS)");
    assert!(c.value("Subject key ID").is_some());
    assert!(c.value("Authority key ID").is_some());

    let d = open("leaf.der", None);
    assert_eq!(d.items[0], f.items[0]);
}

#[test]
fn bundles() {
    let f = open("bundle.pem", None);
    assert_eq!(f.kind, SigningKind::CertificateBundle);
    assert_eq!(f.items.len(), 2);
    let ca = &f.items[1];
    assert_eq!(val(ca, "Self-signed"), "Yes");
    assert_eq!(val(ca, "Basic constraints (critical)"), "CA, path length 1");
    assert_eq!(val(ca, "Public key"), "EC P-256 (256 bits)");
    assert_eq!(
        val(ca, "SHA-256 fingerprint"),
        "E3:8B:F0:81:68:86:6E:DE:32:E4:02:78:D4:F0:0A:90:6C:8A:DB:3E:14:F2:75:14:F9:38:22:2D:6F:D7:78:41"
    );
    assert_eq!(
        val(ca, "Key usage (critical)"),
        "Certificate signing, CRL signing"
    );

    let p7 = open("bundle.p7b", None);
    assert_eq!(p7.kind, SigningKind::Pkcs7);
    assert_eq!(
        p7.items
            .iter()
            .filter(|i| i.kind == SigningItemKind::Certificate)
            .count(),
        2
    );
    assert_eq!(p7.items[0].title, f.items[0].title);
}

#[test]
fn request_and_crl() {
    let f = open("leaf.csr", None);
    assert_eq!(f.kind, SigningKind::CertificateRequest);
    let r = &f.items[0];
    assert_eq!(r.title, "Apple Distribution: Acme Inc (ABCDE12345)");
    assert_eq!(val(r, "Public key"), "RSA 2048 bits");
    assert_eq!(val(r, "SAN DNS"), "example.com, www.example.com");

    let c = open("ca.crl", None);
    assert_eq!(c.kind, SigningKind::RevocationList);
    let l = &c.items[0];
    assert_eq!(val(l, "Revoked certificates"), "1");
    assert_eq!(l.children[0].title, "1234ABCD");
    assert!(l.not_after.is_some());
}

#[test]
fn public_and_private_keys() {
    let spki = "15:83:59:5D:A9:F7:8A:4A:D6:16:08:87:37:AB:37:38:E8:59:B8:25:98:2F:79:AD:07:B2:53:A6:50:95:07:C7";
    let p = open("leaf-pub.pem", None);
    assert_eq!(p.kind, SigningKind::PublicKey);
    assert_eq!(val(&p.items[0], "Public key SHA-256"), spki);

    for name in ["leaf.key", "leaf-rsa.key"] {
        let f = open(name, None);
        assert_eq!(f.kind, SigningKind::PrivateKey, "{name}");
        let k = &f.items[0];
        assert_eq!(k.kind, SigningItemKind::PrivateKey);
        assert_eq!(val(k, "Private key"), "present (not shown)");
        assert_eq!(val(k, "Protection"), "unencrypted");
        assert_eq!(val(k, "Public key SHA-256"), spki, "{name}");
        // Last bytes of the DER are CRT parameters: private.
        let der = pem_der(name);
        assert_no_secret(&f, &der[der.len() - 40..]);
        assert_no_secret(&f, &der[der.len() - 300..der.len() - 260]);
    }

    let ec = open("ca-ec.key", None);
    let bundle = open("bundle.pem", None);
    assert_eq!(val(&ec.items[0], "Algorithm"), "EC P-256");
    assert_eq!(
        val(&ec.items[0], "Public key SHA-256"),
        val(&bundle.items[1], "Public key SHA-256")
    );
    let der = pem_der("ca-ec.key");
    // ECPrivateKey: SEQUENCE, INTEGER 1, OCTET STRING (32 private bytes).
    assert_no_secret(&ec, &der[7..39]);

    let ed = open("ed.key", None);
    assert_eq!(val(&ed.items[0], "Algorithm"), "Ed25519");
    let der = pem_der("ed.key");
    assert_no_secret(&ed, &der[der.len() - 32..]);

    let enc = open("leaf-enc.key", None);
    let prot = val(&enc.items[0], "Protection");
    assert!(
        prot.contains("PBES2") && prot.contains("AES-256-CBC"),
        "{prot}"
    );
    assert!(prot.contains("iterations"), "{prot}");
}

#[test]
fn pkcs12_locked_and_unlocked() {
    let bytes = fx("default.p12");
    let locked = read("default.p12", &bytes, None).unwrap();
    assert!(locked.needs_password());
    assert_eq!(locked.password, PasswordState::Needed);
    assert_eq!(locked.raw.as_deref(), Some(&bytes));
    assert!(
        !locked
            .items
            .iter()
            .any(|i| i.kind == SigningItemKind::Certificate)
    );
    let key = locked
        .items
        .iter()
        .find(|i| i.kind == SigningItemKind::PrivateKey)
        .unwrap();
    assert_eq!(val(key, "Friendly name"), "Acme Distribution");
    assert_eq!(val(key, "Bag"), "pkcs8ShroudedKeyBag");
    assert!(val(key, "Encryption").contains("PBES2"));
    let container = &locked.items[0];
    assert_eq!(val(container, "MAC algorithm"), "HMAC-SHA-256");
    assert!(val(container, "Encrypted parts").contains("AES-256-CBC"));

    let wrong = read("default.p12", &bytes, Some("nope")).unwrap();
    assert_eq!(wrong.password, PasswordState::Wrong);
    assert!(wrong.needs_password());
    assert!(wrong.raw.is_some());

    let open_ = read("default.p12", &bytes, Some("secret")).unwrap();
    assert_eq!(open_.password, PasswordState::Unlocked);
    assert!(!open_.needs_password());
    assert!(open_.raw.is_none());
    assert_eq!(val(&open_.items[0], "Integrity"), "verified");
    let certs: Vec<&SigningItem> = open_
        .items
        .iter()
        .filter(|i| i.kind == SigningItemKind::Certificate)
        .collect();
    assert_eq!(certs.len(), 2);
    assert_eq!(val(certs[0], "Friendly name"), "Acme Distribution");
    let key = open_
        .items
        .iter()
        .find(|i| i.kind == SigningItemKind::PrivateKey)
        .unwrap();
    assert_eq!(
        val(key, "Pairs with"),
        "Apple Distribution: Acme Inc (ABCDE12345)"
    );
    assert!(key.value("Local key ID").is_some());
    let leaf_der = pem_der("leaf.key");
    assert_no_secret(&open_, &leaf_der[leaf_der.len() - 40..]);
    // The password is not kept anywhere.
    assert!(!format!("{open_:?}").contains("secret\""));

    // Locked and unlocked sides align on the key.
    let rows = align(Some(&locked), Some(&open_));
    assert!(
        rows.iter()
            .any(|r| r.kind == SigningItemKind::Certificate && r.status == RowStatus::Added)
    );
}

#[test]
fn pkcs12_variants() {
    let legacy = open("legacy.p12", Some("secret"));
    assert_eq!(legacy.password, PasswordState::Unlocked);
    assert!(val(&legacy.items[0], "Encrypted parts").contains("pbeWithSHAAnd40BitRC2-CBC"));
    assert_eq!(
        legacy
            .items
            .iter()
            .filter(|i| i.kind == SigningItemKind::Certificate)
            .count(),
        1
    );
    let key = legacy
        .items
        .iter()
        .find(|i| i.kind == SigningItemKind::PrivateKey)
        .unwrap();
    assert!(val(key, "Encryption").contains("TripleDES"));

    let nopass = open("nopass.p12", None);
    assert_eq!(nopass.password, PasswordState::None);
    assert!(
        nopass
            .items
            .iter()
            .any(|i| i.kind == SigningItemKind::Certificate)
    );

    let pbmac = open("pbmac1.p12", Some("secret"));
    assert!(val(&pbmac.items[0], "MAC algorithm").starts_with("PBMAC1"));
    assert_eq!(val(&pbmac.items[0], "Integrity"), "verified");
    assert_eq!(pbmac.password, PasswordState::Unlocked);
    assert_eq!(
        open("pbmac1.p12", Some("bad")).password,
        PasswordState::Wrong
    );

    let certonly = open("certonly.p12", Some("secret"));
    assert_eq!(val(&certonly.items[0], "Private keys"), "0");
    assert_eq!(certonly.items[1].title, "Test Root CA");

    // openssl -nomac leaves the certificates unencrypted.
    let nomac = open("nomac.p12", None);
    assert_eq!(nomac.password, PasswordState::None);
    assert_eq!(val(&nomac.items[0], "Integrity"), "no MAC");
    assert!(
        nomac
            .items
            .iter()
            .any(|i| i.kind == SigningItemKind::Certificate)
    );

    // A PKCS#12 file named like a Java keystore.
    let jks = read("store.jks", &fx("default.p12"), Some("secret")).unwrap();
    assert_eq!(jks.kind, SigningKind::Pkcs12);
}

#[test]
fn ssh_public_keys() {
    let ed = open("id_ed25519.pub", None);
    assert_eq!(ed.kind, SigningKind::SshPublicKey);
    let k = &ed.items[0];
    assert_eq!(k.title, "alice@example.com");
    assert_eq!(val(k, "Type"), "ssh-ed25519");
    assert_eq!(
        val(k, "Fingerprint"),
        "SHA256:wakyYQEcGTJx6TKYmLs1uBLGzIzZVBkbwyQVMkBjexE"
    );

    let rsa = open("id_rsa.pub", None);
    assert_eq!(val(&rsa.items[0], "Size"), "2048 bits");
    assert_eq!(
        val(&rsa.items[0], "Fingerprint"),
        "SHA256:k3P/wZrxL1ilFSrx1GemfXg+xh6GmIhT6QHCZHs/Cok"
    );
    assert_eq!(
        val(&rsa.items[0], "MD5 fingerprint"),
        "MD5:70:63:d6:8b:47:9a:06:5d:38:3c:a5:02:7a:8f:d9:ba"
    );

    let ec = open("id_ecdsa.pub", None);
    assert_eq!(val(&ec.items[0], "Size"), "P-256 (256 bits)");

    let r = open("rsa.rfc4716", None);
    assert_eq!(
        val(&r.items[0], "Fingerprint"),
        val(&rsa.items[0], "Fingerprint")
    );

    let cert = open("id_ed25519-cert.pub", None);
    assert_eq!(cert.kind, SigningKind::SshCertificate);
    let c = &cert.items[0];
    assert_eq!(c.kind, SigningItemKind::SshCertificate);
    assert_eq!(val(c, "Key ID"), "alice-cert");
    assert_eq!(val(c, "Serial"), "42");
    assert_eq!(val(c, "Certificate type"), "user");
    assert_eq!(val(c, "Principals"), "alice, bob");
    assert_eq!(val(c, "Critical options"), "force-command=/bin/true");
    assert!(val(c, "Extensions").contains("permit-pty"));
    assert_eq!(
        val(c, "Fingerprint"),
        "SHA256:wakyYQEcGTJx6TKYmLs1uBLGzIzZVBkbwyQVMkBjexE"
    );
    assert_eq!(
        val(c, "Signing CA"),
        "ssh-ed25519 SHA256:/jsKahvf6i8GIk5NzSbtAQSAQc1mtfPYoFbSY/X6kMY"
    );
    assert!(c.not_before.is_some() && c.not_after.is_some());
}

#[test]
fn ssh_key_files() {
    let a = open("authorized_keys", None);
    assert_eq!(a.kind, SigningKind::SshAuthorizedKeys);
    assert_eq!(a.items.len(), 3);
    assert!(val(&a.items[1], "Options").starts_with("from=\"10.0.0.0/8,192.168.1.*\""));
    assert!(val(&a.items[1], "Options").ends_with("no-pty"));
    assert_eq!(a.items[2].kind, SigningItemKind::SshCertificate);
    assert!(
        a.notes
            .iter()
            .any(|n| n.contains("Lines not read as keys: 1"))
    );

    let k = open("known_hosts", None);
    assert_eq!(k.kind, SigningKind::SshKnownHosts);
    assert_eq!(k.items.len(), 4);
    assert_eq!(k.items[0].title, "github.com");
    assert_eq!(val(&k.items[1], "Hosts"), "[git.example.com]:2222,10.0.0.5");
    assert_eq!(val(&k.items[2], "Marker"), "@cert-authority");
    assert_eq!(val(&k.items[3], "Marker"), "@revoked");

    let h = open("known_hosts_hashed", None);
    assert_eq!(h.items.len(), 5);
    assert_eq!(h.items[0].title, "Hashed host");
    assert!(val(&h.items[0], "Hosts").starts_with("hashed host (|1|"));

    let s = open("allowed_signers", None);
    assert_eq!(s.kind, SigningKind::SshAllowedSigners);
    assert_eq!(
        val(&s.items[1], "Principals"),
        "bob@example.com,carol@example.com"
    );
    assert_eq!(
        val(&s.items[1], "Options"),
        "namespaces=\"git\",valid-after=\"20250101\""
    );
}

/// The 32 byte Ed25519 seed inside an unencrypted openssh-key-v1 file.
fn openssh_seed(data: &[u8]) -> Vec<u8> {
    let mut p = b"openssh-key-v1\0".len();
    let u32_at = |p: usize| u32::from_be_bytes(data[p..p + 4].try_into().unwrap()) as usize;
    let string = |p: &mut usize| {
        let n = u32_at(*p);
        let s = data[*p + 4..*p + 4 + n].to_vec();
        *p += 4 + n;
        s
    };
    string(&mut p);
    string(&mut p);
    string(&mut p);
    p += 4;
    string(&mut p);
    let private = string(&mut p);
    // checkints, key type, public key, then the 64 byte private string.
    let mut q = 8;
    let n = u32::from_be_bytes(private[q..q + 4].try_into().unwrap()) as usize;
    q += 4 + n;
    let n = u32::from_be_bytes(private[q..q + 4].try_into().unwrap()) as usize;
    q += 4 + n + 4;
    private[q..q + 32].to_vec()
}

#[test]
fn ssh_private_keys() {
    let f = open("id_ed25519", None);
    assert_eq!(f.kind, SigningKind::SshPrivateKey);
    let k = &f.items[0];
    assert_eq!(k.kind, SigningItemKind::PrivateKey);
    assert_eq!(val(k, "Protection"), "unencrypted");
    assert_eq!(
        val(k, "Fingerprint"),
        "SHA256:wakyYQEcGTJx6TKYmLs1uBLGzIzZVBkbwyQVMkBjexE"
    );
    assert!(
        !format!("{f:?}").contains("alice@example.com"),
        "comment is in the private section"
    );
    assert_no_secret(&f, &openssh_seed(&pem_der("id_ed25519")));

    let p = open("id_protected", None);
    let prot = val(&p.items[0], "Protection");
    assert!(
        prot.contains("bcrypt") && prot.contains("aes256-ctr"),
        "{prot}"
    );
}

#[test]
fn pgp_keys() {
    let fp = "pgp:D107F7BE56A9D7DE3735D45CC6B04B852FD7E4E5";
    for name in ["pgp-pub.gpg", "pgp-pub.asc"] {
        let f = open(name, None);
        assert_eq!(f.kind, SigningKind::PgpPublicKeys, "{name}");
        assert_eq!(f.items.len(), 1);
        let k = &f.items[0];
        assert_eq!(k.key, fp);
        assert_eq!(k.title, "Hand Made <hand@example.com>");
        assert_eq!(val(k, "Version"), "4");
        assert_eq!(val(k, "Algorithm"), "EdDSA (legacy)");
        assert_eq!(val(k, "Size"), "Ed25519");
        assert_eq!(val(k, "Key ID"), "C6B04B852FD7E4E5");
        assert_eq!(
            val(k, "Fingerprint"),
            "D107 F7BE 56A9 D7DE 3735 D45C C6B0 4B85 2FD7 E4E5"
        );
        assert_eq!(val(k, "Created"), "2024-10-04 14:47:28 UTC");
        assert_eq!(val(k, "Expires"), "2025-10-04 14:47:28 UTC");
        assert_eq!(k.not_after, Some(0x6700_0000 + 365 * 86400));
        assert_eq!(val(k, "Usage"), "certify, sign");
        assert_eq!(val(k, "Preferred ciphers"), "AES-256, AES-192, AES-128");
        assert_eq!(val(k, "Preferred hashes"), "SHA-512, SHA-384, SHA-256");
        assert_eq!(val(k, "Preferred compression"), "ZLIB, ZIP");
        assert_eq!(val(k, "User IDs"), "2");
        assert_eq!(val(k, "User attributes"), "1");
        let uid = &k.children[0];
        assert_eq!(uid.kind, SigningItemKind::PgpUserId);
        assert_eq!(val(uid, "Primary"), "yes");
        assert_eq!(val(uid, "Third-party certifications"), "1");
        let old = &k.children[1];
        assert_eq!(
            val(old, "Revoked"),
            "user ID no longer valid: left the company"
        );
        assert_eq!(k.children[2].kind, SigningItemKind::PgpUserAttribute);
        let sub = &k.children[3];
        assert_eq!(sub.kind, SigningItemKind::PgpSubkey);
        assert_eq!(val(sub, "Algorithm"), "ECDH");
        assert_eq!(val(sub, "Size"), "Curve25519");
        assert_eq!(val(sub, "Usage"), "encrypt communications, encrypt storage");
        assert!(f.notes.iter().any(|n| n.contains("not verified")));
    }
}

#[test]
fn pgp_secret_and_v6() {
    let secret = [0xA5u8, 0x5A].repeat(16);
    let p = open("pgp-sec-protected.asc", None);
    assert_eq!(p.kind, SigningKind::PgpSecretKeys);
    assert_eq!(
        p.items[0].key,
        "pgp:D107F7BE56A9D7DE3735D45CC6B04B852FD7E4E5"
    );
    assert_eq!(
        val(&p.items[0], "Secret key"),
        "secret key material present (protected)"
    );
    assert_no_secret(&p, &secret);
    let c = open("pgp-sec-clear.gpg", None);
    assert_eq!(
        val(&c.items[0], "Secret key"),
        "secret key material present (unprotected)"
    );
    assert_eq!(
        val(&c.items[0].children[3], "Secret key"),
        "secret key material present (unprotected)"
    );
    assert_no_secret(&c, &secret);

    let v6 = open("pgp-v6.gpg", None);
    let k = &v6.items[0];
    assert_eq!(val(k, "Version"), "6");
    assert_eq!(val(k, "Algorithm"), "Ed25519");
    assert_eq!(
        k.key,
        "pgp:026C0B6C6D85283E163506C5EADF44310807EFFD1ACA81993E906348E7212194"
    );
    assert_eq!(val(k, "Key ID"), "026C0B6C6D85283E");
    assert_eq!(val(k, "Usage"), "certify, sign");
    assert_eq!(
        val(k, "Preferred AEAD ciphersuites"),
        "AES-256/OCB, AES-128/OCB"
    );
    assert_eq!(k.title, "Six <six@example.com>");
}

// DER helpers for building fixtures in code.

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 128 {
        out.push(n as u8);
    } else if n < 256 {
        out.extend([0x81, n as u8]);
    } else {
        out.extend([0x82, (n >> 8) as u8, n as u8]);
    }
    out.extend_from_slice(content);
    out
}

/// BER with an indefinite length.
fn ber(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag, 0x80];
    out.extend_from_slice(content);
    out.extend([0, 0]);
    out
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// (header length, content length) of the element at `data`.
fn tlv(data: &[u8]) -> (usize, usize) {
    let l = data[1];
    if l < 0x80 {
        (2, l as usize)
    } else {
        let n = (l & 0x7F) as usize;
        let len = data[2..2 + n]
            .iter()
            .fold(0usize, |a, &b| (a << 8) | b as usize);
        (2 + n, len)
    }
}

/// Children of a constructed element as whole elements.
fn kids(data: &[u8]) -> Vec<&[u8]> {
    let (h, n) = tlv(data);
    let mut c = &data[h..h + n];
    let mut out = Vec::new();
    while !c.is_empty() {
        let (h, n) = tlv(c);
        out.push(&c[..h + n]);
        c = &c[h + n..];
    }
    out
}

const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AppIDName</key>
	<string>Acme App</string>
	<key>ApplicationIdentifierPrefix</key>
	<array><string>ABCDE12345</string></array>
	<key>CreationDate</key>
	<date>2026-01-01T00:00:00Z</date>
	<key>Platform</key>
	<array><string>iOS</string><string>xrOS</string></array>
	<key>IsXcodeManaged</key>
	<false/>
	<key>DeveloperCertificates</key>
	<array><data>CERT</data></array>
	<key>Entitlements</key>
	<dict>
		<key>application-identifier</key>
		<string>ABCDE12345.com.acme.app</string>
		<key>keychain-access-groups</key>
		<array><string>ABCDE12345.*</string><string>com.apple.token</string></array>
		<key>get-task-allow</key>
		<true/>
		<key>com.apple.developer.team-identifier</key>
		<string>ABCDE12345</string>
		<key>com.apple.nested</key>
		<dict><key>inner</key><integer>7</integer></dict>
	</dict>
	<key>ExpirationDate</key>
	<date>2027-01-01T00:00:00Z</date>
	<key>Name</key>
	<string>Acme Development</string>
	<key>ProvisionedDevices</key>
	<array>
		<string>00008030-001A2B3C4D5E6F70</string>
		<string>00008101-000A1B2C3D4E5F60</string>
	</array>
	<key>TeamIdentifier</key>
	<array><string>ABCDE12345</string></array>
	<key>TeamName</key>
	<string>Acme Inc</string>
	<key>TimeToLive</key>
	<integer>365</integer>
	<key>UUID</key>
	<string>8C2B6F4E-1D3A-4B5C-9E8F-7A6B5C4D3E2F</string>
	<key>Version</key>
	<integer>1</integer>
</dict>
</plist>"#;

fn build_profile(devices: bool) -> Vec<u8> {
    let leaf = fx("leaf.der");
    let ca = pem_der("ca.pem");
    let b64 = base64::engine::general_purpose::STANDARD.encode(&leaf);
    let mut plist = PLIST.replace("CERT", &b64);
    if !devices {
        let a = plist.find("\t<key>ProvisionedDevices</key>").unwrap();
        let b = plist.find("\t<key>TeamIdentifier</key>").unwrap();
        plist.replace_range(a..b, "");
    }
    // Leaf issuer Name and serial for the signer identifier.
    let tbs = kids(&leaf)[0];
    let t = kids(tbs);
    let (serial, issuer) = (t[1], t[3]);
    let oid = |s: &[u8]| der(0x06, s);
    let data_oid = oid(&[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x01]);
    let signed_oid = oid(&[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02]);
    let sha256 = der(
        0x30,
        &oid(&[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01]),
    );
    // eContent as a chunked (constructed) OCTET STRING.
    let bytes = plist.as_bytes();
    let (a, b) = bytes.split_at(bytes.len() / 2);
    let econtent = ber(0x24, &cat(&[&der(0x04, a), &der(0x04, b)]));
    let eci = ber(0x30, &cat(&[&data_oid, &ber(0xA0, &econtent)]));
    let signer = der(
        0x30,
        &cat(&[
            &der(0x02, &[1]),
            &der(0x30, &cat(&[issuer, serial])),
            &sha256,
            &der(
                0x30,
                &oid(&[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02]),
            ),
            &der(0x04, &[0x55; 64]),
        ]),
    );
    let sd = ber(
        0x30,
        &cat(&[
            &der(0x02, &[1]),
            &der(0x31, &sha256),
            &eci,
            &ber(0xA0, &cat(&[&ca, &leaf])),
            &der(0x31, &signer),
        ]),
    );
    ber(0x30, &cat(&[&signed_oid, &ber(0xA0, &sd)]))
}

#[test]
fn provisioning_profile() {
    let bytes = build_profile(true);
    let f = read("embedded.mobileprovision", &bytes, None).unwrap();
    assert_eq!(f.kind, SigningKind::ProvisioningProfile);
    let p = &f.items[0];
    assert_eq!(p.kind, SigningItemKind::Profile);
    assert_eq!(p.title, "Acme Development");
    assert_eq!(val(p, "Type"), "Development");
    assert_eq!(val(p, "App ID name"), "Acme App");
    assert_eq!(val(p, "Application identifier"), "ABCDE12345.com.acme.app");
    assert_eq!(val(p, "Team name"), "Acme Inc");
    assert_eq!(val(p, "Team identifier"), "ABCDE12345");
    assert_eq!(val(p, "UUID"), "8C2B6F4E-1D3A-4B5C-9E8F-7A6B5C4D3E2F");
    assert_eq!(val(p, "Platforms"), "iOS, xrOS");
    assert_eq!(val(p, "Expires"), "2027-01-01 00:00:00 UTC");
    assert_eq!(p.not_after, Some(1_798_761_600));
    assert_eq!(val(p, "Time to live"), "365 days");
    assert_eq!(val(p, "Provisioned devices"), "2");
    assert_eq!(
        val(p, "Signed by"),
        "Apple Distribution: Acme Inc (ABCDE12345)"
    );
    let ent = &p.children[0];
    assert_eq!(ent.kind, SigningItemKind::Entitlements);
    assert_eq!(
        val(ent, "keychain-access-groups"),
        "ABCDE12345.*, com.apple.token"
    );
    assert_eq!(val(ent, "get-task-allow"), "true");
    assert_eq!(val(ent, "com.apple.nested.inner"), "7");
    let certs = &p.children[1];
    assert_eq!(
        certs.children[0].title,
        "Apple Distribution: Acme Inc (ABCDE12345)"
    );
    assert_eq!(
        val(&certs.children[0], "SHA-1 fingerprint"),
        "8A:18:42:31:61:B0:1D:CC:FE:44:4D:96:1A:4A:60:65:A3:73:AD:61"
    );
    let devices = &p.children[2];
    assert_eq!(devices.children.len(), 2);
    assert_eq!(devices.children[0].key, "device:00008030-001A2B3C4D5E6F70");
    assert_eq!(f.items[1].kind, SigningItemKind::Signer);

    // App Store variant: no devices, no get-task-allow needed.
    let store = read("x.mobileprovision", &build_profile(false), None).unwrap();
    // get-task-allow is true but there are no devices: App Store by Xcode's rule.
    assert_eq!(val(&store.items[0], "Type"), "App Store");

    let rows = align(Some(&store), Some(&f));
    assert!(
        rows.iter()
            .any(|r| r.key == "devices" && r.status == RowStatus::Added)
    );
    assert!(rows.iter().any(|r| r.key == "profile"
        && matches!(&r.status, RowStatus::Changed { fields } if fields.contains(&"Type".to_string()))));
}

fn jutf(s: &str) -> Vec<u8> {
    let mut v = (s.len() as u16).to_be_bytes().to_vec();
    v.extend_from_slice(s.as_bytes());
    v
}

fn build_jks(magic: u32, secret_entry: bool) -> Vec<u8> {
    let leaf = fx("leaf.der");
    let ca = pem_der("ca.pem");
    let cert = |c: &[u8]| cat(&[&jutf("X.509"), &(c.len() as u32).to_be_bytes(), c]);
    // EncryptedPrivateKeyInfo with the JKS key protector OID and fake bytes.
    let jks_oid = der(
        0x06,
        &[0x2B, 0x06, 0x01, 0x04, 0x01, 0x2A, 0x02, 0x11, 0x01, 0x01],
    );
    let protected = der(
        0x30,
        &cat(&[
            &der(0x30, &cat(&[&jks_oid, &[0x05, 0x00]])),
            &der(0x04, &[0xA5; 48]),
        ]),
    );
    let ts = 1_760_000_000_000u64.to_be_bytes();
    let mut out = magic.to_be_bytes().to_vec();
    out.extend(2u32.to_be_bytes());
    out.extend((if secret_entry { 3u32 } else { 2u32 }).to_be_bytes());
    // Trusted certificate.
    out.extend(2u32.to_be_bytes());
    out.extend(jutf("root"));
    out.extend(ts);
    out.extend(cert(&ca));
    if secret_entry {
        out.extend(3u32.to_be_bytes());
        out.extend(jutf("aes"));
        out.extend(ts);
        out.extend([0xAC, 0xED, 0x00, 0x05]);
    }
    // Private key entry with a two certificate chain.
    out.extend(1u32.to_be_bytes());
    out.extend(jutf("signing"));
    out.extend(ts);
    out.extend((protected.len() as u32).to_be_bytes());
    out.extend(&protected);
    out.extend(2u32.to_be_bytes());
    out.extend(cert(&leaf));
    out.extend(cert(&ca));
    out.extend([0u8; 20]);
    out
}

#[test]
fn java_keystores() {
    let f = read("release.jks", &build_jks(0xFEED_FEED, false), None).unwrap();
    assert_eq!(f.kind, SigningKind::JavaKeystore);
    assert_eq!(val(&f.items[0], "Format"), "JKS");
    assert_eq!(val(&f.items[0], "Entries"), "2");
    let root = &f.items[1];
    assert_eq!(root.key, "alias:root");
    assert_eq!(val(root, "Type"), "Trusted certificate");
    assert_eq!(val(root, "Created"), "2025-10-09 08:53:20 UTC");
    assert!(root.not_after.is_some());
    let key = &f.items[2];
    assert_eq!(val(key, "Type"), "Private key entry");
    assert_eq!(val(key, "Key protection"), "JKS key protector");
    assert_eq!(key.children.len(), 2);
    assert_eq!(
        key.children[0].title,
        "Apple Distribution: Acme Inc (ABCDE12345)"
    );
    assert_no_secret(&f, &[0xA5; 24]);

    let j = read("x.jceks", &build_jks(0xCECE_CECE, true), None).unwrap();
    assert_eq!(val(&j.items[0], "Format"), "JCEKS");
    assert_eq!(j.items.len(), 3);
    assert_eq!(val(&j.items[2], "Type"), "Secret key");
    assert!(j.notes.iter().any(|n| n.contains("not read")));

    let bks = read("x.bks", b"\0\0\0\x02junk", None).unwrap();
    assert_eq!(bks.kind, SigningKind::Unsupported);
}

#[test]
fn align_bundles() {
    let old = open("bundle.pem", None);
    let new = open("leaf.pem", None);
    let rows = align(Some(&old), Some(&new));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].status, RowStatus::Unchanged);
    assert_eq!(rows[1].status, RowStatus::Removed);
    assert_eq!(rows[1].title, "Test Root CA");
}

#[test]
fn not_signing_material() {
    assert!(read("notes.pem", b"hello world\n", None).is_none());
    assert!(read("x.der", &[0x30, 0x03, 0x02, 0x01, 0x05], None).is_none());
    assert!(read("x.p12", &[], None).is_none());
}

const FIXTURES: &[&str] = &[
    "ca.pem",
    "leaf.pem",
    "leaf.der",
    "bundle.pem",
    "bundle.p7b",
    "leaf.csr",
    "ca.crl",
    "leaf.key",
    "leaf-enc.key",
    "leaf-rsa.key",
    "ca-ec.key",
    "ed.key",
    "leaf-pub.pem",
    "default.p12",
    "legacy.p12",
    "nopass.p12",
    "pbmac1.p12",
    "certonly.p12",
    "nomac.p12",
    "id_ed25519",
    "id_ed25519.pub",
    "id_rsa.pub",
    "id_ecdsa.pub",
    "id_protected",
    "id_ed25519-cert.pub",
    "rsa.rfc4716",
    "authorized_keys",
    "known_hosts",
    "known_hosts_hashed",
    "allowed_signers",
    "pgp-pub.gpg",
    "pgp-pub.asc",
    "pgp-sec-protected.asc",
    "pgp-sec-clear.gpg",
    "pgp-v6.gpg",
];

#[test]
fn truncated_prefixes_do_not_panic() {
    let mut inputs: Vec<(String, Vec<u8>)> =
        FIXTURES.iter().map(|n| (n.to_string(), fx(n))).collect();
    inputs.push(("a.mobileprovision".into(), build_profile(true)));
    inputs.push(("a.jks".into(), build_jks(0xFEED_FEED, false)));
    inputs.push(("a.jceks".into(), build_jks(0xCECE_CECE, true)));
    for (name, bytes) in &inputs {
        for end in 0..bytes.len() {
            let _ = read(name, &bytes[..end], None);
        }
        // Single byte corruptions at a stride (wider for PKCS#12, where
        // every readable variant runs the key derivation).
        let step = if name.ends_with(".p12") { 401 } else { 11 };
        for i in (0..bytes.len()).step_by(step) {
            let mut b = bytes.clone();
            b[i] ^= 0xFF;
            let _ = read(name, &b, Some("secret"));
        }
    }
}

#[test]
fn deep_nesting_and_absurd_lengths() {
    let mut deep = Vec::new();
    for _ in 0..5000 {
        deep.extend([0x30, 0x80]);
    }
    assert!(read("x.der", &deep, None).is_none());
    let absurd = [0x30, 0x84, 0xFF, 0xFF, 0xFF, 0xFF, 0x02, 0x01, 0x03];
    assert!(read("x.p12", &absurd, None).is_none());
    // PGP partial body lengths that run past the end.
    let pgp = [0xC6, 0xEF, 0x04, 0x00];
    let _ = read("x.gpg", &pgp, None);
}

#[test]
fn large_inputs() {
    let pem = String::from_utf8(fx("leaf.pem")).unwrap();
    let bundle = pem.repeat(3500);
    let line = String::from_utf8(fx("id_ed25519.pub")).unwrap();
    let hosts: String = (0..3000)
        .map(|i| format!("host{i}.example.com {line}"))
        .collect();
    // About 5 MB; in release this reads in well under 100 ms.
    let b = read("big.pem", bundle.as_bytes(), None).unwrap();
    let h = read("known_hosts", hosts.as_bytes(), None).unwrap();
    let rows = align(Some(&b), Some(&b));
    assert_eq!(b.items.len(), 3500);
    assert_eq!(h.items.len(), 3000);
    assert_eq!(rows.len(), 3500);
}
