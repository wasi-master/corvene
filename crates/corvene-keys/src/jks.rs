//! Java keystores (JKS `FEEDFEED`, JCEKS `CECECECE`). Entries are listed
//! with their certificates; protected keys are never decrypted and the
//! integrity hash (keyed by the store password) is not checked.

use crate::der;
use crate::{oid, util, x509};
use corvene_models::signing::{SigningFile, SigningItem, SigningItemKind, SigningKind};

/// Most entries read.
const MAX_ENTRIES: u32 = 100_000;

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
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
    /// Java modified UTF-8 string.
    fn utf(&mut self) -> Option<String> {
        let n = usize::from(self.u16()?);
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn cert(&mut self, version: u32) -> Option<(String, &'a [u8])> {
        let ty = if version == 2 {
            self.utf()?
        } else {
            "X.509".into()
        };
        let n = self.u32()? as usize;
        Some((ty, self.take(n)?))
    }
}

fn cert_child(ty: &str, bytes: &[u8]) -> SigningItem {
    if ty == "X.509" {
        x509::cert_item(bytes, true)
    } else {
        let mut i = SigningItem::new(
            SigningItemKind::Certificate,
            format!("cert:{ty}"),
            ty.to_string(),
        );
        i.field("Type", ty.to_string());
        i.field("Size", format!("{} bytes", bytes.len()));
        i
    }
}

/// Reads a JKS / JCEKS keystore. `None` when the magic does not match.
pub fn read(bytes: &[u8]) -> Option<SigningFile> {
    let mut c = Cursor {
        data: bytes,
        pos: 0,
    };
    let magic = c.u32()?;
    let format = match magic {
        0xFEED_FEED => "JKS",
        0xCECE_CECE => "JCEKS",
        _ => return None,
    };
    let mut file = SigningFile {
        kind: SigningKind::JavaKeystore,
        ..SigningFile::default()
    };
    file.notes.push(
        "The keystore integrity hash needs the store password and is not checked; private keys are never decrypted"
            .into(),
    );
    let mut head = SigningItem::new(
        SigningItemKind::Container,
        "keystore",
        format!("{format} keystore"),
    );
    head.field("Format", format);
    let version = c.u32();
    let count = c.u32();
    if let Some(v) = version {
        head.field("Version", v.to_string());
    }
    if let Some(n) = count {
        head.field("Entries", n.to_string());
    }
    file.items.push(head);
    let (Some(version), Some(count)) = (version, count) else {
        file.notes.push("The keystore is truncated".into());
        return Some(file);
    };
    for _ in 0..count.min(MAX_ENTRIES) {
        let Some(tag) = c.u32() else {
            file.notes.push("The keystore is truncated".into());
            break;
        };
        let (Some(alias), Some(ms)) = (c.utf(), c.u64()) else {
            file.notes.push("The keystore is truncated".into());
            break;
        };
        let mut item = SigningItem::new(
            SigningItemKind::KeystoreEntry,
            format!("alias:{alias}"),
            alias.clone(),
        );
        item.field("Alias", alias.clone());
        let secs = i64::try_from(ms / 1000).unwrap_or(i64::MAX);
        item.field("Created", util::utc(secs));
        let ok = match tag {
            1 => {
                item.field("Type", "Private key entry");
                (|| {
                    let n = c.u32()? as usize;
                    let protected = c.take(n)?;
                    item.field("Private key", "present (not shown)");
                    let alg = der::top(protected)
                        .and_then(|t| t.children().next())
                        .and_then(|a| a.children().next())
                        .and_then(|o| der::oid(&o))
                        .map_or_else(|| "unknown".into(), |o| oid::label(&o));
                    item.field("Key protection", alg);
                    let chain = c.u32()?;
                    item.field("Chain length", chain.to_string());
                    for _ in 0..chain.min(MAX_ENTRIES) {
                        let (ty, bytes) = c.cert(version)?;
                        item.children.push(cert_child(&ty, bytes));
                    }
                    if let Some(first) = item.children.first() {
                        let subject = first.value("Subject").map(str::to_string);
                        let (nb, na) = (first.not_before, first.not_after);
                        if let Some(v) = subject {
                            item.field("Subject", v);
                        }
                        item.not_after = na;
                        item.not_before = nb;
                    }
                    Some(())
                })()
            }
            2 => {
                item.field("Type", "Trusted certificate");
                (|| {
                    let (ty, bytes) = c.cert(version)?;
                    let child = cert_child(&ty, bytes);
                    item.not_after = child.not_after;
                    item.not_before = child.not_before;
                    item.fields.extend(child.fields);
                    Some(())
                })()
            }
            3 => {
                item.field("Type", "Secret key");
                item.field("Secret key", "present (not shown)");
                file.items.push(item);
                file.notes.push(
                    "Entries after a secret key (a Java serialized object) are not read".into(),
                );
                return Some(file);
            }
            other => {
                file.notes
                    .push(format!("Unknown entry type {other}; the rest is not read"));
                return Some(file);
            }
        };
        file.items.push(item);
        if ok.is_none() {
            file.notes.push("The keystore is truncated".into());
            break;
        }
    }
    Some(file)
}
