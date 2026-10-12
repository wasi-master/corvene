//! Apple provisioning profiles: a CMS SignedData around an XML plist.

use crate::plist::Value;
use crate::{pkcs7, plist, util, x509};
use corvene_models::signing::{SigningFile, SigningItem, SigningItemKind, SigningKind};

/// Most devices listed one by one.
const MAX_DEVICES: usize = 20_000;

fn flatten(prefix: &str, v: &Value, out: &mut Vec<(String, String)>, depth: usize) {
    match v {
        Value::Dict(d) if depth < 8 => {
            for (k, v) in d {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out, depth + 1);
            }
        }
        _ => out.push((prefix.to_string(), v.display())),
    }
}

fn date(v: Option<&Value>) -> Option<(String, i64)> {
    let s = v?.str()?;
    Some((s.to_string(), util::parse_iso(s)?))
}

/// Profile type as Xcode derives it.
fn profile_type(p: &Value, platforms: &str) -> &'static str {
    let all = p
        .get("ProvisionsAllDevices")
        .and_then(Value::bool)
        .unwrap_or(false);
    let devices = p.get("ProvisionedDevices").is_some();
    let ent = p.get("Entitlements");
    let task_allow = ent
        .and_then(|e| {
            e.get("get-task-allow")
                .or_else(|| e.get("com.apple.security.get-task-allow"))
        })
        .and_then(Value::bool)
        .unwrap_or(false);
    if all {
        if platforms.contains("OSX") {
            "Developer ID"
        } else {
            "Enterprise (in-house)"
        }
    } else if devices {
        if task_allow { "Development" } else { "Ad Hoc" }
    } else {
        "App Store"
    }
}

/// Reads a provisioning profile. `None` when not a CMS-wrapped plist.
pub fn read(bytes: &[u8]) -> Option<SigningFile> {
    let sd = pkcs7::parse(bytes)?;
    let content = sd.content.as_deref()?;
    let text = std::str::from_utf8(content).ok()?;
    if !text.contains("<plist") {
        return None;
    }
    let p = plist::parse(text)?;
    let mut file = SigningFile {
        kind: SigningKind::ProvisioningProfile,
        ..SigningFile::default()
    };
    file.notes
        .push("The profile signature is not verified".into());

    let s = |k: &str| p.get(k).map(Value::display);
    let name = s("Name").unwrap_or_else(|| "Provisioning profile".into());
    let mut item = SigningItem::new(SigningItemKind::Profile, "profile", name.clone());
    let platforms = s("Platform").unwrap_or_default();
    item.field("Name", name);
    item.field("Type", profile_type(&p, &platforms));
    if let Some(v) = s("AppIDName") {
        item.field("App ID name", v);
    }
    let ent = p.get("Entitlements");
    let app_id = ent
        .and_then(|e| {
            e.get("application-identifier")
                .or_else(|| e.get("com.apple.application-identifier"))
        })
        .map(Value::display);
    if let Some(v) = app_id {
        item.field("Application identifier", v);
    }
    if let Some(v) = s("TeamName") {
        item.field("Team name", v);
    }
    if let Some(v) = s("TeamIdentifier") {
        item.field("Team identifier", v);
    }
    if let Some(v) = s("UUID") {
        item.field("UUID", v);
    }
    if !platforms.is_empty() {
        item.field("Platforms", platforms);
    }
    if let Some((_, secs)) = date(p.get("CreationDate")) {
        item.field("Created", util::utc(secs));
        item.not_before = Some(secs);
    }
    if let Some((_, secs)) = date(p.get("ExpirationDate")) {
        item.field("Expires", util::utc(secs));
        item.not_after = Some(secs);
    }
    if let Some(v) = s("TimeToLive") {
        item.field("Time to live", format!("{v} days"));
    }
    if let Some(v) = s("ProvisionsAllDevices") {
        item.field("Provisions all devices", v);
    }
    if let Some(v) = s("IsXcodeManaged") {
        item.field("Xcode managed", v);
    }
    if let Some(v) = s("Version") {
        item.field("Version", v);
    }
    let devices = match p.get("ProvisionedDevices") {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    };
    if p.get("ProvisionedDevices").is_some() {
        item.field("Provisioned devices", devices.len().to_string());
    }
    if let Some(signer) = pkcs7::signer_cert(&sd) {
        item.field("Signed by", signer.title());
    }

    if let Some(e) = ent {
        let mut flat = Vec::new();
        flatten("", e, &mut flat, 0);
        let mut group = SigningItem::new(
            SigningItemKind::Entitlements,
            "entitlements",
            "Entitlements",
        );
        for (k, v) in flat {
            group.field(k, v);
        }
        item.children.push(group);
    }
    if let Some(Value::Array(certs)) = p.get("DeveloperCertificates") {
        let mut group = SigningItem::new(
            SigningItemKind::Certificates,
            "certificates",
            "Developer certificates",
        );
        group.field("Count", certs.len().to_string());
        for c in certs {
            if let Value::Data(d) = c {
                group.children.push(x509::cert_item(d, true));
            }
        }
        item.children.push(group);
    }
    if !devices.is_empty() {
        let mut group =
            SigningItem::new(SigningItemKind::Devices, "devices", "Provisioned devices");
        group.field("Count", devices.len().to_string());
        for d in devices.iter().take(MAX_DEVICES) {
            let udid = d.display();
            group.children.push(SigningItem::new(
                SigningItemKind::Device,
                format!("device:{udid}"),
                udid,
            ));
        }
        item.children.push(group);
    }
    file.items.push(item);
    if let Some(signer) = pkcs7::signer_cert(&sd) {
        let mut s = signer.summary_item();
        s.kind = SigningItemKind::Signer;
        s.key = format!("signer:{}", s.key);
        file.items.push(s);
    }
    Some(file)
}
