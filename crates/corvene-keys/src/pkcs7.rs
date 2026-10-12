//! PKCS#7 / CMS SignedData (BER): certificates, the signer and the
//! encapsulated content.

use crate::der::{self, Tlv};
use crate::x509;

const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";

/// What a SignedData holds.
pub struct SignedData<'a> {
    /// Encapsulated content (joined if BER chunked).
    pub content: Option<Vec<u8>>,
    pub certs: Vec<&'a [u8]>,
    pub crls: usize,
    /// Signer identities: (issuer DER content, serial content) or a
    /// subject key id.
    pub signers: Vec<Signer<'a>>,
}

pub enum Signer<'a> {
    IssuerSerial(&'a [u8], &'a [u8]),
    KeyId(&'a [u8]),
}

/// Parses a ContentInfo holding SignedData.
pub fn parse(data: &[u8]) -> Option<SignedData<'_>> {
    let top = der::top(data)?;
    let mut r = top.children();
    let ty = r.next().and_then(|o| der::oid(&o))?;
    if ty != SIGNED_DATA {
        return None;
    }
    let sd = r.next().filter(|c| c.is_context(0))?.children().next()?;
    let mut s = sd.children();
    s.next().filter(|v| v.id == der::INTEGER)?;
    s.next().filter(|d| d.id == der::SET)?;
    let eci = s.next().filter(|e| e.id == der::SEQUENCE)?;
    let mut e = eci.children();
    let _content_type = e.next();
    let content = e
        .next()
        .filter(|c| c.is_context(0))
        .and_then(|c| c.children().next())
        .map(|o| o.octets().into_owned());
    let mut out = SignedData {
        content,
        certs: Vec::new(),
        crls: 0,
        signers: Vec::new(),
    };
    for t in s {
        if t.is_context(0) {
            out.certs.extend(
                t.children()
                    .filter(|c| c.id == der::SEQUENCE)
                    .map(|c| c.raw),
            );
        } else if t.is_context(1) {
            out.crls += t.children().count();
        } else if t.id == der::SET {
            for si in t.children() {
                let mut x = si.children();
                let _v = x.next();
                match x.next() {
                    Some(sid) if sid.id == der::SEQUENCE => {
                        let mut i = sid.children();
                        if let (Some(iss), Some(ser)) = (i.next(), i.next()) {
                            out.signers
                                .push(Signer::IssuerSerial(iss.content, ser.content));
                        }
                    }
                    Some(sid) if sid.is_context(0) => out.signers.push(Signer::KeyId(sid.content)),
                    _ => {}
                }
            }
        }
    }
    Some(out)
}

/// The certificate that signed, matched by issuer and serial or key id.
pub fn signer_cert<'a>(sd: &SignedData<'a>) -> Option<x509::Cert<'a>> {
    let signer = sd.signers.first()?;
    sd.certs
        .iter()
        .filter_map(|c| x509::parse_cert(c))
        .find(|c| match signer {
            Signer::IssuerSerial(iss, ser) => {
                c.issuer_raw == *iss && c.serial == x509::serial_hex(ser)
            }
            Signer::KeyId(id) => c.extensions.iter().any(|e| {
                let mut r = e.children();
                r.next().and_then(|o| der::oid(&o)).as_deref() == Some("2.5.29.14")
                    && r.last()
                        .and_then(|v| der::top(v.content))
                        .is_some_and(|k: Tlv| k.content == *id)
            }),
        })
}
