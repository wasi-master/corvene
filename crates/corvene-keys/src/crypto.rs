//! Hashes, HMAC, PBKDF2, the PKCS#12 key derivation and CBC decryption,
//! used only to open the certificate parts of PKCS#12 files.

use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, InnerIvInit, KeyIvInit};
use sha2::Digest;

/// Highest iteration count honoured (each try costs about a second at
/// this level).
pub const MAX_ITERATIONS: u64 = 1_000_000;

/// Hash functions used by PKCS#12 and PBES2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlg {
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlg {
    /// From a digest or HMAC OID.
    pub fn from_oid(oid: &str) -> Option<Self> {
        Some(match oid {
            "1.3.14.3.2.26" | "1.2.840.113549.2.7" => Self::Sha1,
            "2.16.840.1.101.3.4.2.4" | "1.2.840.113549.2.8" => Self::Sha224,
            "2.16.840.1.101.3.4.2.1" | "1.2.840.113549.2.9" => Self::Sha256,
            "2.16.840.1.101.3.4.2.2" | "1.2.840.113549.2.10" => Self::Sha384,
            "2.16.840.1.101.3.4.2.3" | "1.2.840.113549.2.11" => Self::Sha512,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "SHA-1",
            Self::Sha224 => "SHA-224",
            Self::Sha256 => "SHA-256",
            Self::Sha384 => "SHA-384",
            Self::Sha512 => "SHA-512",
        }
    }
}

trait Hasher: Clone {
    const OUT: usize;
    const BLOCK: usize;
    fn start() -> Self;
    fn feed(&mut self, data: &[u8]);
    fn finish(self) -> Vec<u8>;
}

impl Hasher for sha1_smol::Sha1 {
    const OUT: usize = 20;
    const BLOCK: usize = 64;
    fn start() -> Self {
        sha1_smol::Sha1::new()
    }
    fn feed(&mut self, data: &[u8]) {
        self.update(data);
    }
    fn finish(self) -> Vec<u8> {
        self.digest().bytes().to_vec()
    }
}

macro_rules! sha2_hasher {
    ($t:ty, $out:expr, $block:expr) => {
        impl Hasher for $t {
            const OUT: usize = $out;
            const BLOCK: usize = $block;
            fn start() -> Self {
                <$t>::new()
            }
            fn feed(&mut self, data: &[u8]) {
                Digest::update(self, data);
            }
            fn finish(self) -> Vec<u8> {
                self.finalize().to_vec()
            }
        }
    };
}

sha2_hasher!(sha2::Sha224, 28, 64);
sha2_hasher!(sha2::Sha256, 32, 64);
sha2_hasher!(sha2::Sha384, 48, 128);
sha2_hasher!(sha2::Sha512, 64, 128);

struct Hmac<H: Hasher> {
    inner: H,
    outer: H,
}

impl<H: Hasher> Hmac<H> {
    fn new(key: &[u8]) -> Self {
        let mut k = if key.len() > H::BLOCK {
            let mut h = H::start();
            h.feed(key);
            h.finish()
        } else {
            key.to_vec()
        };
        k.resize(H::BLOCK, 0);
        let mut inner = H::start();
        inner.feed(&k.iter().map(|b| b ^ 0x36).collect::<Vec<_>>());
        let mut outer = H::start();
        outer.feed(&k.iter().map(|b| b ^ 0x5C).collect::<Vec<_>>());
        Self { inner, outer }
    }

    fn mac(&self, parts: &[&[u8]]) -> Vec<u8> {
        let mut i = self.inner.clone();
        for p in parts {
            i.feed(p);
        }
        let mut o = self.outer.clone();
        o.feed(&i.finish());
        o.finish()
    }
}

fn hmac_g<H: Hasher>(key: &[u8], msg: &[u8]) -> Vec<u8> {
    Hmac::<H>::new(key).mac(&[msg])
}

fn pbkdf2_g<H: Hasher>(password: &[u8], salt: &[u8], iterations: u64, len: usize) -> Vec<u8> {
    let mac = Hmac::<H>::new(password);
    let mut out = Vec::with_capacity(len + H::OUT);
    let mut block: u32 = 1;
    while out.len() < len {
        let mut u = mac.mac(&[salt, &block.to_be_bytes()]);
        let mut t = u.clone();
        for _ in 1..iterations {
            u = mac.mac(&[&u]);
            for (a, b) in t.iter_mut().zip(&u) {
                *a ^= b;
            }
        }
        out.extend(t);
        block += 1;
    }
    out.truncate(len);
    out
}

/// RFC 7292 appendix B key derivation.
fn p12kdf_g<H: Hasher>(
    id: u8,
    password: &[u8],
    salt: &[u8],
    iterations: u64,
    len: usize,
) -> Vec<u8> {
    let v = H::BLOCK;
    let fill = |s: &[u8]| -> Vec<u8> {
        if s.is_empty() {
            return Vec::new();
        }
        let n = v * s.len().div_ceil(v);
        s.iter().cycle().take(n).copied().collect()
    };
    let d = vec![id; v];
    let mut i = fill(salt);
    i.extend(fill(password));
    let mut out = Vec::with_capacity(len + H::OUT);
    while out.len() < len {
        let mut h = H::start();
        h.feed(&d);
        h.feed(&i);
        let mut a = h.finish();
        for _ in 1..iterations {
            let mut h = H::start();
            h.feed(&a);
            a = h.finish();
        }
        out.extend_from_slice(&a);
        if out.len() >= len {
            break;
        }
        let b: Vec<u8> = a.iter().cycle().take(v).copied().collect();
        for chunk in i.chunks_mut(v) {
            let mut carry: u16 = 1;
            for k in (0..chunk.len()).rev() {
                let s = u16::from(chunk[k]) + u16::from(b[k]) + carry;
                chunk[k] = s as u8;
                carry = s >> 8;
            }
        }
    }
    out.truncate(len);
    out
}

macro_rules! dispatch {
    ($alg:expr, $f:ident ( $($a:expr),* )) => {
        match $alg {
            HashAlg::Sha1 => $f::<sha1_smol::Sha1>($($a),*),
            HashAlg::Sha224 => $f::<sha2::Sha224>($($a),*),
            HashAlg::Sha256 => $f::<sha2::Sha256>($($a),*),
            HashAlg::Sha384 => $f::<sha2::Sha384>($($a),*),
            HashAlg::Sha512 => $f::<sha2::Sha512>($($a),*),
        }
    };
}

pub fn hmac(alg: HashAlg, key: &[u8], msg: &[u8]) -> Vec<u8> {
    dispatch!(alg, hmac_g(key, msg))
}

pub fn pbkdf2(alg: HashAlg, password: &[u8], salt: &[u8], iterations: u64, len: usize) -> Vec<u8> {
    dispatch!(alg, pbkdf2_g(password, salt, iterations.max(1), len))
}

pub fn p12kdf(
    alg: HashAlg,
    id: u8,
    password: &[u8],
    salt: &[u8],
    iterations: u64,
    len: usize,
) -> Vec<u8> {
    dispatch!(alg, p12kdf_g(id, password, salt, iterations.max(1), len))
}

pub fn out_len(alg: HashAlg) -> usize {
    match alg {
        HashAlg::Sha1 => 20,
        HashAlg::Sha224 => 28,
        HashAlg::Sha256 => 32,
        HashAlg::Sha384 => 48,
        HashAlg::Sha512 => 64,
    }
}

/// Block ciphers used by PKCS#12 encryption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cipher {
    Aes128,
    Aes192,
    Aes256,
    TdesEde3,
    TdesEde2,
    /// RC2 with this effective key length in bits.
    Rc2(usize),
}

fn cbc_g<C>(c: cbc::Decryptor<C>, data: &[u8]) -> Option<Vec<u8>>
where
    C: cbc::cipher::BlockDecryptMut + cbc::cipher::BlockCipher,
{
    let mut buf = data.to_vec();
    let n = c.decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?.len();
    buf.truncate(n);
    Some(buf)
}

/// CBC decryption with PKCS#7 padding; `None` on bad padding or sizes.
pub fn cbc_decrypt(cipher: Cipher, key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    match cipher {
        Cipher::Aes128 => cbc_g(
            cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv).ok()?,
            data,
        ),
        Cipher::Aes192 => cbc_g(
            cbc::Decryptor::<aes::Aes192>::new_from_slices(key, iv).ok()?,
            data,
        ),
        Cipher::Aes256 => cbc_g(
            cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv).ok()?,
            data,
        ),
        Cipher::TdesEde3 => cbc_g(
            cbc::Decryptor::<des::TdesEde3>::new_from_slices(key, iv).ok()?,
            data,
        ),
        Cipher::TdesEde2 => cbc_g(
            cbc::Decryptor::<des::TdesEde2>::new_from_slices(key, iv).ok()?,
            data,
        ),
        Cipher::Rc2(bits) => {
            if key.is_empty() || key.len() > 128 {
                return None;
            }
            let inner = rc2::Rc2::new_with_eff_key_len(key, bits);
            cbc_g(
                cbc::Decryptor::<rc2::Rc2>::inner_iv_slice_init(inner, iv).ok()?,
                data,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    #[test]
    fn hmac_vectors() {
        // RFC 4231 test case 2 and RFC 2202 test case 2.
        assert_eq!(
            hex(&hmac(
                HashAlg::Sha256,
                b"Jefe",
                b"what do ya want for nothing?"
            )),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac(
                HashAlg::Sha1,
                b"Jefe",
                b"what do ya want for nothing?"
            )),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }

    #[test]
    fn pbkdf2_vector() {
        // RFC 6070 test case 2.
        assert_eq!(
            hex(&pbkdf2(HashAlg::Sha1, b"password", b"salt", 2, 20)),
            "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957"
        );
    }
}
