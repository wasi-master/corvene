//! Names for the object identifiers shown in certificates and keys.

/// Known OIDs: (dotted, name).
const NAMES: &[(&str, &str)] = &[
    // Public key algorithms
    ("1.2.840.113549.1.1.1", "RSA"),
    ("1.2.840.113549.1.1.10", "RSA-PSS"),
    ("1.2.840.113549.1.1.7", "RSA-OAEP"),
    ("1.2.840.10045.2.1", "EC"),
    ("1.2.840.10040.4.1", "DSA"),
    ("1.2.840.10046.2.1", "DH"),
    ("1.3.101.110", "X25519"),
    ("1.3.101.111", "X448"),
    ("1.3.101.112", "Ed25519"),
    ("1.3.101.113", "Ed448"),
    ("2.16.840.1.101.3.4.3.17", "ML-DSA-44"),
    ("2.16.840.1.101.3.4.3.18", "ML-DSA-65"),
    ("2.16.840.1.101.3.4.3.19", "ML-DSA-87"),
    ("2.16.840.1.101.3.4.4.1", "ML-KEM-512"),
    ("2.16.840.1.101.3.4.4.2", "ML-KEM-768"),
    ("2.16.840.1.101.3.4.4.3", "ML-KEM-1024"),
    // Signature algorithms
    ("1.2.840.113549.1.1.4", "md5WithRSAEncryption"),
    ("1.2.840.113549.1.1.5", "sha1WithRSAEncryption"),
    ("1.2.840.113549.1.1.11", "sha256WithRSAEncryption"),
    ("1.2.840.113549.1.1.12", "sha384WithRSAEncryption"),
    ("1.2.840.113549.1.1.13", "sha512WithRSAEncryption"),
    ("1.2.840.113549.1.1.14", "sha224WithRSAEncryption"),
    ("1.2.840.10045.4.1", "ecdsa-with-SHA1"),
    ("1.2.840.10045.4.3.1", "ecdsa-with-SHA224"),
    ("1.2.840.10045.4.3.2", "ecdsa-with-SHA256"),
    ("1.2.840.10045.4.3.3", "ecdsa-with-SHA384"),
    ("1.2.840.10045.4.3.4", "ecdsa-with-SHA512"),
    ("1.2.840.10040.4.3", "dsa-with-SHA1"),
    ("2.16.840.1.101.3.4.3.2", "dsa-with-SHA256"),
    // Curves
    ("1.2.840.10045.3.1.7", "P-256"),
    ("1.3.132.0.34", "P-384"),
    ("1.3.132.0.35", "P-521"),
    ("1.3.132.0.10", "secp256k1"),
    ("1.2.840.10045.3.1.1", "P-192"),
    ("1.3.132.0.33", "P-224"),
    ("1.3.36.3.3.2.8.1.1.7", "brainpoolP256r1"),
    ("1.3.36.3.3.2.8.1.1.11", "brainpoolP384r1"),
    ("1.3.36.3.3.2.8.1.1.13", "brainpoolP512r1"),
    ("1.3.6.1.4.1.11591.15.1", "Ed25519"),
    ("1.3.6.1.4.1.3029.1.5.1", "Curve25519"),
    // Hashes and MACs
    ("1.3.14.3.2.26", "SHA-1"),
    ("2.16.840.1.101.3.4.2.1", "SHA-256"),
    ("2.16.840.1.101.3.4.2.2", "SHA-384"),
    ("2.16.840.1.101.3.4.2.3", "SHA-512"),
    ("2.16.840.1.101.3.4.2.4", "SHA-224"),
    ("1.2.840.113549.2.5", "MD5"),
    ("1.2.840.113549.2.7", "HMAC-SHA1"),
    ("1.2.840.113549.2.8", "HMAC-SHA224"),
    ("1.2.840.113549.2.9", "HMAC-SHA256"),
    ("1.2.840.113549.2.10", "HMAC-SHA384"),
    ("1.2.840.113549.2.11", "HMAC-SHA512"),
    // Encryption
    ("1.2.840.113549.1.5.13", "PBES2"),
    ("1.2.840.113549.1.5.12", "PBKDF2"),
    ("1.2.840.113549.1.5.14", "PBMAC1"),
    ("1.2.840.113549.1.5.3", "pbeWithMD5AndDES-CBC"),
    ("1.2.840.113549.1.5.10", "pbeWithSHA1AndDES-CBC"),
    ("1.2.840.113549.1.12.1.1", "pbeWithSHAAnd128BitRC4"),
    ("1.2.840.113549.1.12.1.2", "pbeWithSHAAnd40BitRC4"),
    ("1.2.840.113549.1.12.1.3", "pbeWithSHAAnd3-KeyTripleDES-CBC"),
    ("1.2.840.113549.1.12.1.4", "pbeWithSHAAnd2-KeyTripleDES-CBC"),
    ("1.2.840.113549.1.12.1.5", "pbeWithSHAAnd128BitRC2-CBC"),
    ("1.2.840.113549.1.12.1.6", "pbeWithSHAAnd40BitRC2-CBC"),
    ("2.16.840.1.101.3.4.1.2", "AES-128-CBC"),
    ("2.16.840.1.101.3.4.1.22", "AES-192-CBC"),
    ("2.16.840.1.101.3.4.1.42", "AES-256-CBC"),
    ("1.2.840.113549.3.7", "DES-EDE3-CBC"),
    ("1.3.14.3.2.7", "DES-CBC"),
    ("1.3.6.1.4.1.42.2.17.1.1", "JKS key protector"),
    ("1.3.6.1.4.1.42.2.19.1", "PBEWithMD5AndTripleDES (JCEKS)"),
    // PKCS#7 / CMS content types
    ("1.2.840.113549.1.7.1", "data"),
    ("1.2.840.113549.1.7.2", "signedData"),
    ("1.2.840.113549.1.7.3", "envelopedData"),
    ("1.2.840.113549.1.7.6", "encryptedData"),
    // Name attributes
    ("2.5.4.3", "CN"),
    ("2.5.4.4", "SN"),
    ("2.5.4.5", "serialNumber"),
    ("2.5.4.6", "C"),
    ("2.5.4.7", "L"),
    ("2.5.4.8", "ST"),
    ("2.5.4.9", "STREET"),
    ("2.5.4.10", "O"),
    ("2.5.4.11", "OU"),
    ("2.5.4.12", "title"),
    ("2.5.4.42", "GN"),
    ("2.5.4.43", "initials"),
    ("2.5.4.46", "dnQualifier"),
    ("2.5.4.65", "pseudonym"),
    ("2.5.4.97", "organizationIdentifier"),
    ("0.9.2342.19200300.100.1.1", "UID"),
    ("0.9.2342.19200300.100.1.25", "DC"),
    ("1.2.840.113549.1.9.1", "emailAddress"),
    ("1.3.6.1.4.1.311.60.2.1.3", "jurisdictionC"),
    ("2.5.4.15", "businessCategory"),
    // Extended key usages
    ("1.3.6.1.5.5.7.3.1", "TLS server authentication"),
    ("1.3.6.1.5.5.7.3.2", "TLS client authentication"),
    ("1.3.6.1.5.5.7.3.3", "Code signing"),
    ("1.3.6.1.5.5.7.3.4", "Email protection"),
    ("1.3.6.1.5.5.7.3.8", "Time stamping"),
    ("1.3.6.1.5.5.7.3.9", "OCSP signing"),
    ("2.5.29.37.0", "Any extended key usage"),
    ("1.3.6.1.4.1.311.10.3.3", "Microsoft server gated crypto"),
    ("1.3.6.1.4.1.311.20.2.2", "Microsoft smart card logon"),
    ("2.16.840.1.113730.4.1", "Netscape server gated crypto"),
    ("1.2.840.113635.100.4.1", "Apple code signing"),
    (
        "1.2.840.113635.100.4.1.1",
        "Apple code signing (development)",
    ),
    (
        "1.2.840.113635.100.4.1.4",
        "Apple code signing (Developer ID)",
    ),
    ("1.2.840.113635.100.4.13", "Apple package signing"),
    ("1.2.840.113635.100.4.14", "Apple system identity"),
    // Certificate policies
    ("2.5.29.32.0", "Any policy"),
    ("2.23.140.1.2.1", "Domain validated"),
    ("2.23.140.1.2.2", "Organization validated"),
    ("2.23.140.1.2.3", "Individual validated"),
    ("2.23.140.1.1", "Extended validation"),
    ("1.2.840.113635.100.5.1", "Apple certificate policy"),
    // Apple certificate markers
    ("1.2.840.113635.100.6.1.2", "Apple Development (iOS)"),
    ("1.2.840.113635.100.6.1.4", "Apple Distribution (iOS)"),
    ("1.2.840.113635.100.6.1.7", "Mac App Store application"),
    ("1.2.840.113635.100.6.1.8", "Mac App Store installer"),
    ("1.2.840.113635.100.6.1.12", "Mac Development"),
    ("1.2.840.113635.100.6.1.13", "Developer ID Application"),
    ("1.2.840.113635.100.6.1.14", "Developer ID Installer"),
    ("1.2.840.113635.100.6.1.33", "Developer ID date"),
    ("1.2.840.113635.100.6.2.1", "Apple WWDR intermediate"),
    ("1.2.840.113635.100.6.2.6", "Developer ID intermediate"),
    (
        "1.2.840.113635.100.6.3.1",
        "Apple Push Notification service (development)",
    ),
    (
        "1.2.840.113635.100.6.3.2",
        "Apple Push Notification service (production)",
    ),
    (
        "1.2.840.113635.100.6.3.6",
        "Apple Push Notification service topics",
    ),
    // Extensions
    ("2.5.29.14", "Subject key identifier"),
    ("2.5.29.15", "Key usage"),
    ("2.5.29.17", "Subject alternative name"),
    ("2.5.29.18", "Issuer alternative name"),
    ("2.5.29.19", "Basic constraints"),
    ("2.5.29.30", "Name constraints"),
    ("2.5.29.31", "CRL distribution points"),
    ("2.5.29.32", "Certificate policies"),
    ("2.5.29.35", "Authority key identifier"),
    ("2.5.29.37", "Extended key usage"),
    ("2.5.29.20", "CRL number"),
    ("2.5.29.21", "Reason code"),
    ("1.3.6.1.5.5.7.1.1", "Authority information access"),
    ("1.3.6.1.4.1.11129.2.4.2", "Certificate transparency SCTs"),
    ("1.3.6.1.5.5.7.1.24", "TLS feature (OCSP must-staple)"),
    ("2.16.840.1.113730.1.1", "Netscape certificate type"),
    ("2.16.840.1.113730.1.13", "Netscape comment"),
    // PKCS#9 / PKCS#12
    ("1.2.840.113549.1.9.20", "friendlyName"),
    ("1.2.840.113549.1.9.21", "localKeyId"),
    ("1.2.840.113549.1.9.14", "extensionRequest"),
    ("1.2.840.113549.1.9.7", "challengePassword"),
    ("1.2.840.113549.1.9.2", "unstructuredName"),
    ("1.2.840.113549.1.9.3", "contentType"),
    ("1.2.840.113549.1.9.4", "messageDigest"),
    ("1.2.840.113549.1.9.5", "signingTime"),
];

/// Name for a dotted OID, if known.
pub fn name(dotted: &str) -> Option<&'static str> {
    static MAP: std::sync::OnceLock<std::collections::HashMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| NAMES.iter().copied().collect())
        .get(dotted)
        .copied()
}

/// Name for a dotted OID, or the OID itself.
pub fn label(dotted: &str) -> String {
    name(dotted).map_or_else(|| dotted.to_string(), str::to_string)
}

/// Bits of a named curve.
pub fn curve_bits(name: &str) -> Option<usize> {
    Some(match name {
        "P-192" => 192,
        "P-224" => 224,
        "P-256" | "secp256k1" | "brainpoolP256r1" | "Ed25519" | "Curve25519" => 256,
        "P-384" | "brainpoolP384r1" => 384,
        "P-521" => 521,
        "brainpoolP512r1" => 512,
        _ => return None,
    })
}
