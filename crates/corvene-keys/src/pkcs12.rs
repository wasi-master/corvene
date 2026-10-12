//! PKCS#12 (`.p12`, `.pfx`). Only certificate containers are decrypted;
//! key bags are listed (present, names, encryption) and never decrypted.

use crate::crypto::{self, Cipher, HashAlg, MAX_ITERATIONS};
use crate::der::{self, Tlv};
use crate::{oid, util, x509};
use corvene_models::signing::{
    PasswordState, SigningFile, SigningItem, SigningItemKind, SigningKind,
};
use std::sync::Arc;

const DATA: &str = "1.2.840.113549.1.7.1";
const ENCRYPTED_DATA: &str = "1.2.840.113549.1.7.6";
const ENVELOPED_DATA: &str = "1.2.840.113549.1.7.3";
const KEY_BAG: &str = "1.2.840.113549.1.12.10.1.1";
const SHROUDED_KEY_BAG: &str = "1.2.840.113549.1.12.10.1.2";
const CERT_BAG: &str = "1.2.840.113549.1.12.10.1.3";
const CRL_BAG: &str = "1.2.840.113549.1.12.10.1.4";
const SECRET_BAG: &str = "1.2.840.113549.1.12.10.1.5";
const SAFE_CONTENTS_BAG: &str = "1.2.840.113549.1.12.10.1.6";

/// A password to try, in both encodings PKCS#12 uses.
struct Pw {
    /// BMPString with terminator (legacy PBE and the classic MAC).
    bmp: Vec<u8>,
    /// UTF-8 (PBES2 and PBMAC1).
    utf8: Vec<u8>,
    given: bool,
}

fn candidates(password: Option<&str>) -> Vec<Pw> {
    let mut out = vec![
        Pw {
            bmp: Vec::new(),
            utf8: Vec::new(),
            given: false,
        },
        Pw {
            bmp: vec![0, 0],
            utf8: Vec::new(),
            given: false,
        },
    ];
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        let mut bmp: Vec<u8> = p.encode_utf16().flat_map(u16::to_be_bytes).collect();
        bmp.extend_from_slice(&[0, 0]);
        out.push(Pw {
            bmp,
            utf8: p.as_bytes().to_vec(),
            given: true,
        });
    }
    out
}

/// PBKDF2 parameters: salt, iterations, key length, PRF.
fn pbkdf2_params<'a>(params: &Tlv<'a>) -> Option<(&'a [u8], u64, Option<u64>, HashAlg)> {
    let mut r = params.children();
    let salt = r.next().filter(|s| s.id == der::OCTET_STRING)?;
    let iter = der::small_int(&r.next()?)?;
    let mut key_len = None;
    let mut prf = HashAlg::Sha1;
    for t in r {
        if t.id == der::INTEGER {
            key_len = der::small_int(&t);
        } else if t.id == der::SEQUENCE {
            prf = t
                .children()
                .next()
                .and_then(|o| der::oid(&o))
                .and_then(|o| HashAlg::from_oid(&o))?;
        }
    }
    Some((salt.content, iter, key_len, prf))
}

fn cipher_for(oid: &str) -> Option<(Cipher, usize)> {
    Some(match oid {
        "2.16.840.1.101.3.4.1.2" => (Cipher::Aes128, 16),
        "2.16.840.1.101.3.4.1.22" => (Cipher::Aes192, 24),
        "2.16.840.1.101.3.4.1.42" => (Cipher::Aes256, 32),
        "1.2.840.113549.3.7" => (Cipher::TdesEde3, 24),
        _ => return None,
    })
}

fn legacy_cipher(oid: &str) -> Option<(Cipher, usize, usize)> {
    Some(match oid {
        "1.2.840.113549.1.12.1.3" => (Cipher::TdesEde3, 24, 8),
        "1.2.840.113549.1.12.1.4" => (Cipher::TdesEde2, 16, 8),
        "1.2.840.113549.1.12.1.5" => (Cipher::Rc2(128), 16, 8),
        "1.2.840.113549.1.12.1.6" => (Cipher::Rc2(40), 5, 8),
        _ => return None,
    })
}

/// Name and iteration count of a password-based encryption AlgorithmIdentifier.
pub fn describe_pbe(alg: &Tlv) -> (String, Option<u64>) {
    let mut r = alg.children();
    let Some(o) = r.next().and_then(|o| der::oid(&o)) else {
        return ("unknown".into(), None);
    };
    let params = r.next();
    if o == "1.2.840.113549.1.5.13" {
        let mut p = params.iter().flat_map(|p| p.children());
        let kdf = p.next();
        let enc = p.next();
        let mut iter = None;
        let mut prf = "HMAC-SHA1".to_string();
        let mut kdf_name = "unknown KDF".to_string();
        if let Some(kdf) = kdf {
            let mut k = kdf.children();
            kdf_name = k
                .next()
                .and_then(|o| der::oid(&o))
                .map_or(kdf_name, |o| oid::label(&o));
            if let Some((_, i, _, h)) = k.next().as_ref().and_then(pbkdf2_params) {
                iter = Some(i);
                prf = format!("HMAC-{}", h.name().replace('-', ""));
            }
        }
        let enc_name = enc
            .and_then(|e| e.children().next())
            .and_then(|o| der::oid(&o))
            .map_or_else(|| "unknown cipher".into(), |o| oid::label(&o));
        return (format!("PBES2 ({kdf_name}-{prf}, {enc_name})"), iter);
    }
    let iter = params
        .and_then(|p| p.children().nth(1))
        .and_then(|i| der::small_int(&i));
    (oid::label(&o), iter)
}

enum Decrypted {
    Ok(Vec<u8>),
    /// Wrong password (bad padding or not DER).
    Failed,
    /// Algorithm or parameters not supported.
    Unsupported(String),
}

fn decrypt(alg: &Tlv, data: &[u8], pw: &Pw) -> Decrypted {
    let mut r = alg.children();
    let Some(o) = r.next().and_then(|o| der::oid(&o)) else {
        return Decrypted::Unsupported("unknown".into());
    };
    let Some(params) = r.next() else {
        return Decrypted::Unsupported(oid::label(&o));
    };
    let plain = if o == "1.2.840.113549.1.5.13" {
        let mut p = params.children();
        let (Some(kdf), Some(enc)) = (p.next(), p.next()) else {
            return Decrypted::Unsupported("PBES2".into());
        };
        let mut k = kdf.children();
        let kdf_oid = k.next().and_then(|o| der::oid(&o));
        let kp = k.next().as_ref().and_then(pbkdf2_params);
        let mut e = enc.children();
        let enc_oid = e.next().and_then(|o| der::oid(&o)).unwrap_or_default();
        let iv = e.next().filter(|i| i.id == der::OCTET_STRING);
        let (
            Some((salt, iter, _, prf)),
            Some((cipher, key_len)),
            Some(iv),
            Some("1.2.840.113549.1.5.12"),
        ) = (kp, cipher_for(&enc_oid), iv, kdf_oid.as_deref())
        else {
            return Decrypted::Unsupported(describe_pbe(alg).0);
        };
        if iter > MAX_ITERATIONS {
            return Decrypted::Unsupported(format!("{} iterations", iter));
        }
        let key = crypto::pbkdf2(prf, &pw.utf8, salt, iter, key_len);
        crypto::cbc_decrypt(cipher, &key, iv.content, data)
    } else if let Some((cipher, key_len, iv_len)) = legacy_cipher(&o) {
        let mut p = params.children();
        let salt = p.next().filter(|s| s.id == der::OCTET_STRING);
        let iter = p.next().and_then(|i| der::small_int(&i)).unwrap_or(1);
        let Some(salt) = salt else {
            return Decrypted::Unsupported(oid::label(&o));
        };
        if iter > MAX_ITERATIONS {
            return Decrypted::Unsupported(format!("{} iterations", iter));
        }
        let key = crypto::p12kdf(HashAlg::Sha1, 1, &pw.bmp, salt.content, iter, key_len);
        let iv = crypto::p12kdf(HashAlg::Sha1, 2, &pw.bmp, salt.content, iter, iv_len);
        crypto::cbc_decrypt(cipher, &key, &iv, data)
    } else {
        return Decrypted::Unsupported(oid::label(&o));
    };
    match plain {
        Some(p) if der::top(&p).is_some_and(|t| t.id == der::SEQUENCE) => Decrypted::Ok(p),
        _ => Decrypted::Failed,
    }
}

struct Mac<'a> {
    alg: String,
    iterations: u64,
    check: MacCheck<'a>,
}

enum MacCheck<'a> {
    Classic {
        hash: HashAlg,
        digest: &'a [u8],
        salt: &'a [u8],
    },
    Pbmac1 {
        prf: HashAlg,
        mac: HashAlg,
        salt: &'a [u8],
        key_len: usize,
        digest: &'a [u8],
    },
    Unsupported,
}

fn parse_mac<'a>(t: &Tlv<'a>) -> Option<Mac<'a>> {
    let mut r = t.children();
    let digest_info = r.next()?;
    let salt = r.next().filter(|s| s.id == der::OCTET_STRING)?;
    let iterations = r.next().and_then(|i| der::small_int(&i)).unwrap_or(1);
    let mut d = digest_info.children();
    let alg = d.next()?;
    let digest = d.next().filter(|s| s.id == der::OCTET_STRING)?;
    let mut a = alg.children();
    let o = a.next().and_then(|o| der::oid(&o))?;
    if o == "1.2.840.113549.1.5.14" {
        let check = (|| {
            let params = a.next()?;
            let mut p = params.children();
            let kdf = p.next()?;
            let scheme = p.next()?;
            let mut k = kdf.children();
            k.next()
                .and_then(|o| der::oid(&o))
                .filter(|o| o == "1.2.840.113549.1.5.12")?;
            let (salt, iter, key_len, prf) = pbkdf2_params(&k.next()?)?;
            let mac = scheme
                .children()
                .next()
                .and_then(|o| der::oid(&o))
                .and_then(|o| HashAlg::from_oid(&o))?;
            let key_len = usize::try_from(key_len.unwrap_or(crypto::out_len(mac) as u64))
                .ok()?
                .min(1024);
            Some((
                MacCheck::Pbmac1 {
                    prf,
                    mac,
                    salt,
                    key_len,
                    digest: digest.content,
                },
                iter,
                format!("PBMAC1 (PBKDF2-HMAC-{}, HMAC-{})", prf.name(), mac.name()),
            ))
        })();
        return Some(match check {
            Some((check, iter, name)) => Mac {
                alg: name,
                iterations: iter,
                check,
            },
            None => Mac {
                alg: "PBMAC1".into(),
                iterations,
                check: MacCheck::Unsupported,
            },
        });
    }
    match HashAlg::from_oid(&o) {
        Some(hash) => Some(Mac {
            alg: format!("HMAC-{}", hash.name()),
            iterations,
            check: MacCheck::Classic {
                hash,
                digest: digest.content,
                salt: salt.content,
            },
        }),
        None => Some(Mac {
            alg: oid::label(&o),
            iterations,
            check: MacCheck::Unsupported,
        }),
    }
}

/// `Some(true)` when the MAC matches with this password, `None` when it
/// cannot be checked.
fn verify_mac(mac: &Mac, auth: &[u8], pw: &Pw) -> Option<bool> {
    if mac.iterations > MAX_ITERATIONS {
        return None;
    }
    match &mac.check {
        MacCheck::Classic { hash, digest, salt } => {
            let key = crypto::p12kdf(
                *hash,
                3,
                &pw.bmp,
                salt,
                mac.iterations,
                crypto::out_len(*hash),
            );
            Some(crypto::hmac(*hash, &key, auth) == *digest)
        }
        MacCheck::Pbmac1 {
            prf,
            mac: m,
            salt,
            key_len,
            digest,
        } => {
            let key = crypto::pbkdf2(*prf, &pw.utf8, salt, mac.iterations, *key_len);
            Some(crypto::hmac(*m, &key, auth) == *digest)
        }
        MacCheck::Unsupported => None,
    }
}

#[derive(Default)]
struct Attrs {
    friendly: Option<String>,
    local_key_id: Option<Vec<u8>>,
}

fn bag_attrs(t: Option<Tlv>) -> Attrs {
    let mut a = Attrs::default();
    let Some(set) = t else { return a };
    for attr in set.children() {
        let mut r = attr.children();
        let Some(o) = r.next().and_then(|o| der::oid(&o)) else {
            continue;
        };
        let Some(v) = r.next().and_then(|s| s.children().next()) else {
            continue;
        };
        match o.as_str() {
            "1.2.840.113549.1.9.20" => a.friendly = der::string(&v),
            "1.2.840.113549.1.9.21" => a.local_key_id = Some(v.content.to_vec()),
            _ => {}
        }
    }
    a
}

struct KeyBag {
    shrouded: bool,
    attrs: Attrs,
    /// Encryption scheme and iterations of a shrouded bag.
    scheme: Option<(String, Option<u64>)>,
    /// Item for an unencrypted key bag (algorithm and public half).
    plain: Option<SigningItem>,
}

#[derive(Default)]
struct Bags {
    certs: Vec<(Vec<u8>, Attrs)>,
    keys: Vec<KeyBag>,
    crls: usize,
    secrets: usize,
    other: usize,
}

fn read_bags(data: &[u8], bags: &mut Bags, depth: usize) {
    if depth > 4 {
        return;
    }
    let Some(seq) = der::top(data) else { return };
    for bag in seq.children() {
        let mut r = bag.children();
        let Some(id) = r.next().and_then(|o| der::oid(&o)) else {
            continue;
        };
        let Some(value) = r
            .next()
            .filter(|v| v.is_context(0))
            .and_then(|v| v.children().next())
        else {
            continue;
        };
        let attrs = bag_attrs(r.next());
        match id.as_str() {
            CERT_BAG => {
                let mut c = value.children();
                let _cert_type = c.next();
                if let Some(der_bytes) = c
                    .next()
                    .filter(|v| v.is_context(0))
                    .and_then(|v| v.children().next())
                {
                    bags.certs.push((der_bytes.octets().into_owned(), attrs));
                }
            }
            SHROUDED_KEY_BAG => {
                let scheme = value.children().next().map(|a| describe_pbe(&a));
                bags.keys.push(KeyBag {
                    shrouded: true,
                    attrs,
                    scheme,
                    plain: None,
                });
            }
            KEY_BAG => {
                bags.keys.push(KeyBag {
                    shrouded: false,
                    attrs,
                    scheme: None,
                    plain: Some(x509::pkcs8_item(&value)),
                });
            }
            CRL_BAG => bags.crls += 1,
            SECRET_BAG => bags.secrets += 1,
            SAFE_CONTENTS_BAG => read_bags(value.raw, bags, depth + 1),
            _ => bags.other += 1,
        }
    }
}

struct Part<'a> {
    kind: &'static str,
    alg: Option<Tlv<'a>>,
    data: std::borrow::Cow<'a, [u8]>,
}

fn parts<'a>(auth: &'a [u8]) -> Vec<Part<'a>> {
    let mut out = Vec::new();
    let Some(seq) = der::top(auth) else {
        return out;
    };
    for ci in seq.children() {
        let mut r = ci.children();
        let Some(ty) = r.next().and_then(|o| der::oid(&o)) else {
            continue;
        };
        let content = r
            .next()
            .filter(|c| c.is_context(0))
            .and_then(|c| c.children().next());
        match (ty.as_str(), content) {
            (DATA, Some(c)) => out.push(Part {
                kind: "data",
                alg: None,
                data: c.octets(),
            }),
            (ENCRYPTED_DATA, Some(c)) => {
                let mut e = c.children();
                let _version = e.next();
                let Some(eci) = e.next() else { continue };
                let mut x = eci.children();
                let _ty = x.next();
                let alg = x.next();
                let enc = x.next().filter(|t| t.is_context(0));
                out.push(Part {
                    kind: "encrypted",
                    alg,
                    data: enc.map(|t| t.octets()).unwrap_or_default(),
                });
            }
            (ENVELOPED_DATA, _) => out.push(Part {
                kind: "enveloped",
                alg: None,
                data: Default::default(),
            }),
            _ => out.push(Part {
                kind: "other",
                alg: None,
                data: Default::default(),
            }),
        }
    }
    out
}

/// Reads a PFX. `None` when the bytes are not PKCS#12.
pub fn read(bytes: &[u8], password: Option<&str>) -> Option<SigningFile> {
    let top = der::top(bytes)?;
    let mut r = top.children();
    if r.next().and_then(|v| der::small_int(&v)) != Some(3) {
        return None;
    }
    let auth_ci = r.next()?;
    let mac = r.next().and_then(|m| parse_mac(&m));
    let mut a = auth_ci.children();
    let ty = a.next().and_then(|o| der::oid(&o))?;
    let mut file = SigningFile {
        kind: SigningKind::Pkcs12,
        ..SigningFile::default()
    };
    if ty != DATA {
        file.notes.push(format!(
            "Public-key protected bundle ({}) not supported",
            oid::label(&ty)
        ));
        return Some(file);
    }
    let auth = a
        .next()
        .filter(|c| c.is_context(0))
        .and_then(|c| c.children().next())?
        .octets();
    let parts = parts(&auth);
    let pws = candidates(password);

    // Which password opens the file.
    let mut chosen: Option<usize> = None;
    let mut checked = false;
    if let Some(m) = &mac {
        for (i, pw) in pws.iter().enumerate() {
            match verify_mac(m, &auth, pw) {
                Some(true) => {
                    chosen = Some(i);
                    checked = true;
                    break;
                }
                Some(false) => checked = true,
                None => break,
            }
        }
    }
    let first_encrypted = parts.iter().find(|p| p.kind == "encrypted");
    let mut unsupported = None;
    if !checked {
        match first_encrypted {
            None => chosen = Some(0),
            Some(p) => {
                for (i, pw) in pws.iter().enumerate() {
                    let Some(alg) = &p.alg else { break };
                    match decrypt(alg, &p.data, pw) {
                        Decrypted::Ok(_) => {
                            chosen = Some(i);
                            break;
                        }
                        Decrypted::Failed => {}
                        Decrypted::Unsupported(s) => {
                            unsupported = Some(s);
                            break;
                        }
                    }
                }
            }
        }
    }

    let mut bags = Bags::default();
    let mut encrypted_parts = Vec::new();
    for p in &parts {
        match p.kind {
            "data" => read_bags(&p.data, &mut bags, 0),
            "encrypted" => {
                let (name, iter) = p.alg.as_ref().map(describe_pbe).unwrap_or_default();
                encrypted_parts.push(match iter {
                    Some(i) => format!("{name}, {i} iterations"),
                    None => name,
                });
                if let (Some(i), Some(alg)) = (chosen, &p.alg) {
                    match decrypt(alg, &p.data, &pws[i]) {
                        Decrypted::Ok(plain) => read_bags(&plain, &mut bags, 0),
                        Decrypted::Failed => file
                            .notes
                            .push("An encrypted part could not be opened".into()),
                        Decrypted::Unsupported(s) => file
                            .notes
                            .push(format!("Encrypted part not supported: {s}")),
                    }
                }
            }
            "enveloped" => encrypted_parts.push("public-key enveloped".into()),
            _ => {}
        }
    }
    if let Some(s) = unsupported {
        file.notes.push(format!("Encryption not supported: {s}"));
    }

    let locked = chosen.is_none() && !encrypted_parts.is_empty();
    file.password = match chosen {
        Some(i) if pws[i].given => PasswordState::Unlocked,
        Some(_) => PasswordState::None,
        None if !locked => PasswordState::None,
        None if password.is_some_and(|p| !p.is_empty()) => PasswordState::Wrong,
        None => PasswordState::Needed,
    };
    if locked {
        file.raw = Some(Arc::new(bytes.to_vec()));
        file.notes
            .push("Certificates are encrypted: enter the password to list them".into());
    }
    if chosen.is_none() && !locked && mac.as_ref().is_some_and(|_| checked) {
        file.notes
            .push("Integrity check (MAC) failed with an empty password".into());
    }
    file.notes
        .push("Private keys are never decrypted or shown".into());

    let mut container = SigningItem::new(SigningItemKind::Container, "pkcs12", "PKCS #12 bundle");
    match &mac {
        Some(m) => {
            container.field("MAC algorithm", m.alg.clone());
            container.field("MAC iterations", m.iterations.to_string());
            let state = if chosen.is_some() && checked {
                "verified"
            } else if checked {
                "not verified (password needed)"
            } else {
                "not checked"
            };
            container.field("Integrity", state);
        }
        None => container.field("Integrity", "no MAC"),
    }
    if !encrypted_parts.is_empty() {
        container.field("Encrypted parts", encrypted_parts.join("; "));
    }
    if !locked {
        container.field("Certificates", bags.certs.len().to_string());
    }
    container.field("Private keys", bags.keys.len().to_string());
    if bags.crls > 0 {
        container.field("CRLs", bags.crls.to_string());
    }
    if bags.secrets > 0 {
        container.field("Secrets", format!("{} (not shown)", bags.secrets));
    }
    if bags.other > 0 {
        container.field("Other bags", bags.other.to_string());
    }
    file.items.push(container);

    let mut cert_titles: Vec<(Option<Vec<u8>>, String, String)> = Vec::new();
    for (der_bytes, attrs) in &bags.certs {
        let mut item = x509::cert_item(der_bytes, false);
        if let Some(f) = &attrs.friendly {
            item.fields.insert(
                0,
                corvene_models::signing::SigningField::new("Friendly name", f.clone()),
            );
        }
        if let Some(k) = &attrs.local_key_id {
            item.field("Local key ID", util::colon_hex(k, true));
        }
        cert_titles.push((
            attrs.local_key_id.clone(),
            item.title.clone(),
            item.key.clone(),
        ));
        file.items.push(item);
    }
    for (n, k) in bags.keys.iter().enumerate() {
        let paired = k
            .attrs
            .local_key_id
            .as_ref()
            .and_then(|id| cert_titles.iter().find(|c| c.0.as_ref() == Some(id)));
        let mut item = k.plain.clone().unwrap_or_default();
        item.kind = SigningItemKind::PrivateKey;
        let name = k
            .attrs
            .friendly
            .clone()
            .or_else(|| paired.map(|p| p.1.clone()));
        item.title = match &name {
            Some(n) => format!("Private key: {n}"),
            None => "Private key".into(),
        };
        item.key = match (paired, &k.attrs.friendly) {
            (Some(p), _) => format!("p12key:{}", p.2),
            (None, Some(f)) => format!("p12key:{f}"),
            _ => format!("p12key:#{n}"),
        };
        let mut fields = vec![corvene_models::signing::SigningField::new(
            "Private key",
            "present (not shown)",
        )];
        fields.push(corvene_models::signing::SigningField::new(
            "Bag",
            if k.shrouded {
                "pkcs8ShroudedKeyBag"
            } else {
                "keyBag (unencrypted)"
            },
        ));
        if let Some((scheme, iter)) = &k.scheme {
            fields.push(corvene_models::signing::SigningField::new(
                "Encryption",
                scheme.clone(),
            ));
            if let Some(i) = iter {
                fields.push(corvene_models::signing::SigningField::new(
                    "Iterations",
                    i.to_string(),
                ));
            }
        }
        if let Some(f) = &k.attrs.friendly {
            fields.push(corvene_models::signing::SigningField::new(
                "Friendly name",
                f.clone(),
            ));
        }
        if let Some(id) = &k.attrs.local_key_id {
            fields.push(corvene_models::signing::SigningField::new(
                "Local key ID",
                util::colon_hex(id, true),
            ));
        }
        if let Some(p) = paired {
            fields.push(corvene_models::signing::SigningField::new(
                "Pairs with",
                p.1.clone(),
            ));
        }
        // Keep the public half fields of an unencrypted key bag.
        fields.extend(
            item.fields
                .drain(..)
                .filter(|f| f.label != "Private key" && f.label != "Protection"),
        );
        item.fields = fields;
        file.items.push(item);
    }
    Some(file)
}
