//! Corvene `1341-signing-material-diff`: certificates, PKCS#12 bundles,
//! provisioning profiles, Java keystores, OpenPGP and SSH keys read as
//! their public fields. Private key material is never decoded or shown.
//!
//! GitHub Desktop shows these as binary or text diffs
//! (`app/src/ui/diff/index.tsx`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// What a signing material file holds as a whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SigningKind {
    /// One X.509 certificate.
    Certificate,
    /// Several certificates (PEM bundle, CA bundle).
    CertificateBundle,
    /// A PKCS#10 certificate signing request.
    CertificateRequest,
    /// An X.509 certificate revocation list.
    RevocationList,
    /// A PKCS#7 / CMS bundle (`.p7b`, `.p7c`).
    Pkcs7,
    /// A PKCS#12 bundle (`.p12`, `.pfx`).
    Pkcs12,
    /// An Apple provisioning profile.
    ProvisioningProfile,
    /// A Java keystore (JKS or JCEKS).
    JavaKeystore,
    /// OpenPGP public keys.
    PgpPublicKeys,
    /// OpenPGP secret keys (only their public parts are shown).
    PgpSecretKeys,
    /// An OpenPGP signature.
    PgpSignature,
    /// An OpenPGP message.
    PgpMessage,
    /// An SSH public key.
    SshPublicKey,
    /// An OpenSSH certificate.
    SshCertificate,
    /// An `authorized_keys` file.
    SshAuthorizedKeys,
    /// A `known_hosts` file.
    SshKnownHosts,
    /// An `allowed_signers` file.
    SshAllowedSigners,
    /// An SSH private key (only its public part is shown).
    SshPrivateKey,
    /// A PEM or DER private key (only its type is shown).
    PrivateKey,
    /// A PEM or DER public key.
    PublicKey,
    /// A PEM file with blocks of several kinds.
    #[default]
    Mixed,
    /// Recognised but not readable (BKS keystores).
    Unsupported,
}

impl SigningKind {
    /// Name shown in the diff header.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Certificate => "Certificate",
            Self::CertificateBundle => "Certificate bundle",
            Self::CertificateRequest => "Certificate signing request",
            Self::RevocationList => "Certificate revocation list",
            Self::Pkcs7 => "PKCS #7 certificate bundle",
            Self::Pkcs12 => "PKCS #12 bundle",
            Self::ProvisioningProfile => "Provisioning profile",
            Self::JavaKeystore => "Java keystore",
            Self::PgpPublicKeys => "OpenPGP public key",
            Self::PgpSecretKeys => "OpenPGP secret key",
            Self::PgpSignature => "OpenPGP signature",
            Self::PgpMessage => "OpenPGP message",
            Self::SshPublicKey => "SSH public key",
            Self::SshCertificate => "SSH certificate",
            Self::SshAuthorizedKeys => "SSH authorized keys",
            Self::SshKnownHosts => "SSH known hosts",
            Self::SshAllowedSigners => "SSH allowed signers",
            Self::SshPrivateKey => "SSH private key",
            Self::PrivateKey => "Private key",
            Self::PublicKey => "Public key",
            Self::Mixed => "Signing material",
            Self::Unsupported => "Unsupported key store",
        }
    }
}

/// Whether reading the file needed a password.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PasswordState {
    /// No password needed (or the empty password worked).
    #[default]
    None,
    /// Locked: a password is needed to read the encrypted parts.
    Needed,
    /// Locked: the password given did not open it.
    Wrong,
    /// Opened with the password given.
    Unlocked,
}

/// What one item in a signing file is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SigningItemKind {
    /// An X.509 certificate.
    #[default]
    Certificate,
    /// A certificate signing request.
    CertificateRequest,
    /// A certificate revocation list.
    RevocationList,
    /// One revoked certificate in a CRL.
    RevokedCertificate,
    /// A bare public key.
    PublicKey,
    /// A private key: only that it is present, and its public half.
    PrivateKey,
    /// A provisioning profile.
    Profile,
    /// A group of entitlements.
    Entitlements,
    /// A group of provisioned devices.
    Devices,
    /// One provisioned device.
    Device,
    /// A group of certificates.
    Certificates,
    /// A keystore entry.
    KeystoreEntry,
    /// An SSH public key line.
    SshKey,
    /// An OpenSSH certificate.
    SshCertificate,
    /// An OpenPGP primary key.
    PgpKey,
    /// An OpenPGP subkey.
    PgpSubkey,
    /// An OpenPGP user ID.
    PgpUserId,
    /// An OpenPGP user attribute (photo).
    PgpUserAttribute,
    /// An OpenPGP signature.
    PgpSignature,
    /// An OpenPGP message.
    PgpMessage,
    /// A CMS signer.
    Signer,
    /// Container facts (PKCS#12 bags, keystore header).
    Container,
}

impl SigningItemKind {
    /// Short label shown next to the item title.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Certificate => "Certificate",
            Self::CertificateRequest => "Signing request",
            Self::RevocationList => "Revocation list",
            Self::RevokedCertificate => "Revoked certificate",
            Self::PublicKey => "Public key",
            Self::PrivateKey => "Private key",
            Self::Profile => "Profile",
            Self::Entitlements => "Entitlements",
            Self::Devices => "Devices",
            Self::Device => "Device",
            Self::Certificates => "Certificates",
            Self::KeystoreEntry => "Keystore entry",
            Self::SshKey => "SSH key",
            Self::SshCertificate => "SSH certificate",
            Self::PgpKey => "OpenPGP key",
            Self::PgpSubkey => "Subkey",
            Self::PgpUserId => "User ID",
            Self::PgpUserAttribute => "User attribute",
            Self::PgpSignature => "Signature",
            Self::PgpMessage => "Message",
            Self::Signer => "Signer",
            Self::Container => "Container",
        }
    }
}

/// One labelled value of an item, in display order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SigningField {
    /// Label ("Subject", "SHA-256 fingerprint").
    pub label: String,
    /// Value as shown.
    pub value: String,
}

impl SigningField {
    /// A field from a label and value.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// One certificate, key, profile, entry or group in a signing file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningItem {
    /// Identity used to pair the item with the other side of the diff
    /// (subject, fingerprint, alias, UDID ...). Unique among siblings in
    /// most files; repeats are paired in order.
    pub key: String,
    /// Title ("Apple Distribution: Acme Inc (ABCDE12345)").
    pub title: String,
    /// What the item is.
    pub kind: SigningItemKind,
    /// Labelled values in display order.
    pub fields: Vec<SigningField>,
    /// Start of validity, unix seconds.
    pub not_before: Option<i64>,
    /// End of validity (expiry), unix seconds.
    pub not_after: Option<i64>,
    /// Nested items (subkeys, user IDs, chain certificates, devices).
    pub children: Vec<SigningItem>,
}

impl SigningItem {
    /// An item with no fields yet.
    pub fn new(kind: SigningItemKind, key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            kind,
            ..Self::default()
        }
    }

    /// Appends a field.
    pub fn field(&mut self, label: impl Into<String>, value: impl Into<String>) {
        self.fields.push(SigningField::new(label, value));
    }

    /// The first value with `label`.
    pub fn value(&self, label: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.value.as_str())
    }

    fn weight(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.key.len()
            + self.title.len()
            + self
                .fields
                .iter()
                .map(|f| std::mem::size_of::<SigningField>() + f.label.len() + f.value.len())
                .sum::<usize>()
            + self.children.iter().map(Self::weight).sum::<usize>()
    }
}

/// One side of a signing material diff.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningFile {
    /// What the file holds.
    pub kind: SigningKind,
    /// Top level items in file order.
    pub items: Vec<SigningItem>,
    /// Remarks about the whole file (signatures not verified, entries not
    /// read ...).
    pub notes: Vec<String>,
    /// Whether a password was needed.
    pub password: PasswordState,
    /// The file bytes while locked, so the file can be read again with a
    /// password the user types. `None` when not locked. The password is
    /// never kept.
    pub raw: Option<Arc<Vec<u8>>>,
}

impl SigningFile {
    /// Rough heap size (diff cache sizing).
    pub fn weight(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.items.iter().map(SigningItem::weight).sum::<usize>()
            + self.notes.iter().map(String::len).sum::<usize>()
            + self.raw.as_ref().map_or(0, |r| r.len())
    }

    /// A password-protected file read without (or with a wrong) password.
    pub fn needs_password(&self) -> bool {
        matches!(self.password, PasswordState::Needed | PasswordState::Wrong)
    }
}

/// How an item compares between the two sides.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RowStatus {
    /// Only in the new file.
    Added,
    /// Only in the old file.
    Removed,
    /// In both, with these field labels changed. Empty when only nested
    /// items changed.
    Changed {
        /// Labels of the fields whose value differs or that one side lacks.
        fields: Vec<String>,
    },
    /// In both and identical, nested items included.
    Unchanged,
}

/// One field of a row with both sides' values.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldRow {
    /// Label.
    pub label: String,
    /// Value in the old file.
    pub old: Option<String>,
    /// Value in the new file.
    pub new: Option<String>,
}

impl FieldRow {
    /// Whether the two sides differ.
    pub fn changed(&self) -> bool {
        self.old != self.new
    }
}

/// One item of the aligned diff, flattened with its depth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRow {
    /// Nesting depth (0 for top level items).
    pub depth: usize,
    /// How the item compares.
    pub status: RowStatus,
    /// What the item is.
    pub kind: SigningItemKind,
    /// Identity key.
    pub key: String,
    /// Title (from the new side when present).
    pub title: String,
    /// Fields with both sides' values, in display order.
    pub fields: Vec<FieldRow>,
    /// Start of validity (new side when present), unix seconds.
    pub not_before: Option<i64>,
    /// Expiry (new side when present), unix seconds.
    pub not_after: Option<i64>,
}

/// Pairs the items of two files by identity and flattens them into rows,
/// children after their parent. A missing side counts as empty.
pub fn align(old: Option<&SigningFile>, new: Option<&SigningFile>) -> Vec<ItemRow> {
    let mut rows = Vec::new();
    let old = old.map_or(&[][..], |f| f.items.as_slice());
    let new = new.map_or(&[][..], |f| f.items.as_slice());
    align_items(old, new, 0, &mut rows);
    rows
}

const MAX_DEPTH: usize = 32;

/// Pairs keys in order: equal keys are matched first come first served,
/// and unmatched old entries are placed before the next matched one.
fn merge_order<K: std::hash::Hash + Eq>(
    old: &[K],
    new: &[K],
) -> Vec<(Option<usize>, Option<usize>)> {
    let mut queues: HashMap<&K, std::collections::VecDeque<usize>> = HashMap::new();
    for (i, k) in old.iter().enumerate() {
        queues.entry(k).or_default().push_back(i);
    }
    let mut matched = vec![false; old.len()];
    let mut pairs = Vec::with_capacity(new.len());
    for (j, k) in new.iter().enumerate() {
        let i = queues.get_mut(k).and_then(|q| q.pop_front());
        if let Some(i) = i {
            matched[i] = true;
        }
        pairs.push((i, j));
    }
    let mut out = Vec::with_capacity(old.len() + new.len());
    let mut next_old = 0;
    for (i, j) in pairs {
        if let Some(i) = i {
            while next_old < i {
                if !matched[next_old] {
                    out.push((Some(next_old), None));
                }
                next_old += 1;
            }
            next_old = next_old.max(i + 1);
        }
        out.push((i, Some(j)));
    }
    for (i, m) in matched.iter().enumerate().skip(next_old) {
        if !m {
            out.push((Some(i), None));
        }
    }
    out
}

/// Field identity: label and occurrence number.
fn field_keys(fields: &[SigningField]) -> Vec<(&str, usize)> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    fields
        .iter()
        .map(|f| {
            let n = seen.entry(f.label.as_str()).or_insert(0);
            *n += 1;
            (f.label.as_str(), *n)
        })
        .collect()
}

fn align_fields(old: &[SigningField], new: &[SigningField]) -> Vec<FieldRow> {
    merge_order(&field_keys(old), &field_keys(new))
        .into_iter()
        .map(|(i, j)| {
            let o = i.and_then(|i| old.get(i));
            let n = j.and_then(|j| new.get(j));
            FieldRow {
                label: n.or(o).map(|f| f.label.clone()).unwrap_or_default(),
                old: o.map(|f| f.value.clone()),
                new: n.map(|f| f.value.clone()),
            }
        })
        .collect()
}

fn one_sided(item: &SigningItem, depth: usize, added: bool, rows: &mut Vec<ItemRow>) {
    if depth > MAX_DEPTH {
        return;
    }
    rows.push(ItemRow {
        depth,
        status: if added {
            RowStatus::Added
        } else {
            RowStatus::Removed
        },
        kind: item.kind,
        key: item.key.clone(),
        title: item.title.clone(),
        fields: item
            .fields
            .iter()
            .map(|f| FieldRow {
                label: f.label.clone(),
                old: (!added).then(|| f.value.clone()),
                new: added.then(|| f.value.clone()),
            })
            .collect(),
        not_before: item.not_before,
        not_after: item.not_after,
    });
    for child in &item.children {
        one_sided(child, depth + 1, added, rows);
    }
}

fn align_items(old: &[SigningItem], new: &[SigningItem], depth: usize, rows: &mut Vec<ItemRow>) {
    if depth > MAX_DEPTH {
        return;
    }
    let ok: Vec<&str> = old.iter().map(|i| i.key.as_str()).collect();
    let nk: Vec<&str> = new.iter().map(|i| i.key.as_str()).collect();
    for pair in merge_order(&ok, &nk) {
        match pair {
            (Some(i), Some(j)) => {
                let (Some(o), Some(n)) = (old.get(i), new.get(j)) else {
                    continue;
                };
                let fields = align_fields(&o.fields, &n.fields);
                let at = rows.len();
                rows.push(ItemRow {
                    depth,
                    status: RowStatus::Unchanged,
                    kind: n.kind,
                    key: n.key.clone(),
                    title: n.title.clone(),
                    fields,
                    not_before: n.not_before,
                    not_after: n.not_after,
                });
                align_items(&o.children, &n.children, depth + 1, rows);
                let changed: Vec<String> = rows[at]
                    .fields
                    .iter()
                    .filter(|f| f.changed())
                    .map(|f| f.label.clone())
                    .collect();
                let nested = rows[at + 1..]
                    .iter()
                    .any(|r| r.status != RowStatus::Unchanged);
                if !changed.is_empty() || nested || o.title != n.title {
                    rows[at].status = RowStatus::Changed { fields: changed };
                }
            }
            (Some(i), None) => {
                if let Some(o) = old.get(i) {
                    one_sided(o, depth, false, rows);
                }
            }
            (None, Some(j)) => {
                if let Some(n) = new.get(j) {
                    one_sided(n, depth, true, rows);
                }
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, value: &str) -> SigningItem {
        let mut i = SigningItem::new(SigningItemKind::Certificate, key, key);
        i.field("Serial", value);
        i
    }

    fn file(items: Vec<SigningItem>) -> SigningFile {
        SigningFile {
            items,
            ..SigningFile::default()
        }
    }

    #[test]
    fn align_pairs_by_key() {
        let mut parent = item("p", "1");
        parent.children.push(item("c", "1"));
        let old = file(vec![item("a", "1"), item("b", "1"), parent.clone()]);
        parent.children[0].fields[0].value = "2".into();
        let new = file(vec![item("b", "2"), item("d", "1"), parent]);
        let rows = align(Some(&old), Some(&new));
        let s: Vec<(&str, &RowStatus)> = rows.iter().map(|r| (r.key.as_str(), &r.status)).collect();
        assert_eq!(s[0], ("a", &RowStatus::Removed));
        assert_eq!(
            s[1],
            (
                "b",
                &RowStatus::Changed {
                    fields: vec!["Serial".into()]
                }
            )
        );
        assert_eq!(s[2], ("d", &RowStatus::Added));
        assert_eq!(s[3], ("p", &RowStatus::Changed { fields: vec![] }));
        assert_eq!(rows[4].depth, 1);
        assert_eq!(rows.len(), 5);
        let only_new = align(None, Some(&new));
        assert!(only_new.iter().all(|r| r.status == RowStatus::Added));
    }

    #[test]
    fn needs_password_states() {
        let mut f = SigningFile::default();
        assert!(!f.needs_password());
        f.password = PasswordState::Wrong;
        assert!(f.needs_password());
    }
}
