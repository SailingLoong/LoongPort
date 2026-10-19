//! Explicit selected-client import only. Never scans native storage or makes IDs.
//! Source: dingminhua/dsh-connect-trae@7a9553eadb7081c6c1f27652a95f97104e8136ed (MIT).
use super::store::{Account, Failure};
#[derive(Clone, Copy)]
pub(crate) enum Edition {
    Cn,
    Global,
}
pub(crate) struct Material {
    pub(super) account: Account,
}
use super::store::{self, Device, Grant, Realm};
use base64::Engine;
use ring::signature::KeyPair;
use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
/// This method is called ONLY after explicit user selection/consent, never on startup.
/// It reads bounded regular files once and never writes the original installation.
pub(crate) fn read_selected(
    storage: &std::path::Path,
    product: &std::path::Path,
    edition: Edition,
) -> Result<Material, Failure> {
    let expected = match edition {
        Edition::Cn => "Trae CN",
        Edition::Global => "Trae",
    };
    if storage.file_name().and_then(|s| s.to_str()) != Some("storage.json")
        || storage
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some("globalStorage")
        || storage
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some("User")
        || storage
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some(expected)
        || product.file_name().and_then(|s| s.to_str()) != Some("product.json")
    {
        return Err(Failure::Unsupported);
    }
    let raw = Zeroizing::new(
        crate::config_file_io::read_regular_file(storage, 4 * 1024 * 1024)
            .map_err(|_| Failure::Storage)?
            .ok_or(Failure::Storage)?,
    );
    let version = crate::config_file_io::read_regular_file(product, 1024 * 1024)
        .map_err(|_| Failure::Storage)?
        .ok_or(Failure::Storage)?;
    parse(
        &raw,
        &version,
        edition,
        match std::env::consts::OS {
            "macos" => "mac",
            other => other,
        },
        "",
    )
}
pub(super) fn value(s: &Value, key: &str, max: usize) -> Result<String, Failure> {
    s.get(key)
        .and_then(Value::as_str)
        .filter(|s| store::text(s, max))
        .map(String::from)
        .ok_or(Failure::Invalid)
}
pub(super) fn pem(s: &str, label: &str) -> Result<Zeroizing<Vec<u8>>, Failure> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let inner = s
        .trim()
        .strip_prefix(&begin)
        .and_then(|s| s.strip_suffix(&end))
        .ok_or(Failure::Invalid)?;
    let encoded = inner
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<String>();
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map(Zeroizing::new)
        .map_err(|_| Failure::Invalid)
}
pub(super) fn key_pair(
    private: &str,
    public: &str,
) -> Result<ring::signature::EcdsaKeyPair, Failure> {
    let der = pem(private, "PRIVATE KEY")?;
    let key = ring::signature::EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
        &der,
        &ring::rand::SystemRandom::new(),
    )
    .map_err(|_| Failure::Invalid)?;
    let mut expected = hex::decode("3059301306072a8648ce3d020106082a8648ce3d030107034200")
        .map_err(|_| Failure::Invalid)?;
    expected.extend_from_slice(key.public_key().as_ref());
    if pem(public, "PUBLIC KEY")?.as_slice() != expected {
        return Err(Failure::Invalid);
    }
    Ok(key)
}
// Native AES-CBC is decoded ONLY for an explicitly selected input snapshot.
// This is the upstream obfuscation codec, never our persistence encryption.
fn local_value(v: &Value) -> Result<Value, Failure> {
    let text = v
        .as_str()
        .filter(|s| s.len() <= 2 * 1024 * 1024)
        .ok_or(Failure::Invalid)?;
    if text.trim_start().starts_with('{') {
        return serde_json::from_str(text).map_err(|_| Failure::Invalid);
    }
    use aes_gcm::aes::{
        cipher::{Array, BlockCipherDecrypt, KeyInit},
        Aes128,
    };
    use sha2::Sha512;
    let raw = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| Failure::Invalid)?,
    );
    if raw.len() <= 102 || (raw.len() - 38) % 16 != 0 {
        return Err(Failure::Invalid);
    }
    let salt: &[u8; 64] = match &raw[..6] {
        [0x74, 0x63, 0x05, 0x10, 0, 0] => &SALT_NORMAL,
        [0x12, 0x39, 0x20, 0x20, 0x02, 0x03] => &SALT_PRIVATE,
        _ => return Err(Failure::Unsupported),
    };
    let mut input = Zeroizing::new(Sha512::digest(&raw[6..38]).to_vec());
    input.extend_from_slice(salt);
    let key = Zeroizing::new(Sha512::digest(&input).to_vec());
    let cipher = Aes128::new_from_slice(&key[..16]).map_err(|_| Failure::Invalid)?;
    let mut previous: [u8; 16] = key[16..32].try_into().map_err(|_| Failure::Invalid)?;
    let mut plain = Zeroizing::new(Vec::with_capacity(raw.len() - 38));
    for chunk in raw[38..].chunks_exact(16) {
        let encrypted: [u8; 16] = chunk.try_into().map_err(|_| Failure::Invalid)?;
        let mut block = Array::from(encrypted);
        cipher.decrypt_block(&mut block);
        plain.extend(block.iter().zip(previous).map(|(a, b)| *a ^ b));
        previous = encrypted;
    }
    let padding = *plain.last().ok_or(Failure::Invalid)? as usize;
    if !(1..=16).contains(&padding)
        || plain.len() < padding
        || !plain[plain.len() - padding..]
            .iter()
            .all(|b| *b as usize == padding)
    {
        return Err(Failure::Invalid);
    }
    let length = plain.len() - padding;
    plain.truncate(length);
    if plain.len() < 64 || Sha512::digest(&plain[64..]).as_slice() != &plain[..64] {
        return Err(Failure::Invalid);
    }
    serde_json::from_slice(&plain[64..]).map_err(|_| Failure::Invalid)
}
const SALT_NORMAL: [u8; 64] = [
    77, 212, 194, 230, 184, 49, 98, 9, 14, 82, 179, 199, 166, 115, 59, 164, 28, 178, 70, 43, 130,
    154, 181, 138, 25, 107, 57, 219, 87, 23, 117, 36, 244, 155, 175, 127, 8, 232, 214, 141, 38,
    167, 46, 55, 193, 169, 90, 47, 31, 5, 165, 24, 146, 174, 242, 148, 151, 50, 182, 42, 56, 170,
    221, 88,
];
const SALT_PRIVATE: [u8; 64] = [
    73, 12, 194, 18, 146, 176, 93, 12, 192, 108, 203, 233, 31, 185, 46, 33, 181, 167, 197, 56, 129,
    29, 200, 94, 39, 169, 110, 40, 16, 78, 28, 246, 156, 113, 102, 239, 171, 35, 40, 7, 57, 13,
    167, 138, 139, 145, 228, 156, 68, 180, 154, 67, 254, 185, 65, 161, 228, 111, 38, 86, 230, 19,
    228, 123,
];
pub(super) fn parse(
    storage: &[u8],
    product: &[u8],
    edition: Edition,
    os: &str,
    os_version: &str,
) -> Result<Material, Failure> {
    if storage.len() > 4 * 1024 * 1024 {
        return Err(Failure::Invalid);
    }
    let doc: Value = serde_json::from_slice(storage).map_err(|_| Failure::Invalid)?;
    let product: Value = serde_json::from_slice(product).map_err(|_| Failure::Invalid)?;
    if product
        .get("packageType")
        .and_then(Value::as_str)
        .is_some_and(|s| s.contains("SOLO") || s.contains("ENTERPRISE"))
    {
        return Err(Failure::Unsupported);
    }
    if matches!(edition, Edition::Global)
        && product.get("packageType").and_then(Value::as_str) != Some("TRAE_I18N")
    {
        return Err(Failure::Unsupported);
    }
    let auth = local_value(&doc["iCubeAuthInfo://icube.cloudide"])?;
    let uid = value(&auth, "userId", 256)?;
    if auth.pointer("/account/tenant_id").is_some()
        || auth.get("scope").and_then(Value::as_str) == Some("saas")
    {
        return Err(Failure::Unsupported);
    }
    let host = value(&auth, "host", 256)?;
    let region = auth
        .pointer("/userRegion/region")
        .or_else(|| auth.get("userRegion"))
        .and_then(Value::as_str)
        .ok_or(Failure::Invalid)?
        .to_ascii_uppercase();
    let realm = match (edition, region.as_str(), host.as_str()) {
        (Edition::Cn, "CN", "https://api.trae.cn") => Realm::Cn,
        (Edition::Global, "SG", "https://growsg-normal.trae.ai") => Realm::Sg,
        (Edition::Global, "US", "https://growsg-normal.trae.ai") => Realm::Us,
        _ => return Err(Failure::Unsupported),
    };
    let devices = doc
        .as_object()
        .ok_or(Failure::Invalid)?
        .keys()
        .filter_map(|k| {
            k.strip_prefix("iCubeAuthInfo://icube-dc:")
                .map(|id| (k, id))
        })
        .collect::<Vec<_>>();
    if devices.len() != 1 || !store::text(devices[0].1, 256) {
        return Err(Failure::Invalid);
    }
    let keys = local_value(&doc[devices[0].0])?;
    let private_key = keys["privateKeyPEM"]
        .as_str()
        .filter(|s| s.len() <= 16384)
        .ok_or(Failure::Invalid)?
        .to_string();
    let public_key = keys["publicKeyPEM"]
        .as_str()
        .filter(|s| s.len() <= 16384)
        .ok_or(Failure::Invalid)?
        .to_string();
    key_pair(&private_key, &public_key)?;
    let expires = auth
        .get("expiredAt")
        .or_else(|| auth.get("expiresAt"))
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .ok_or(Failure::Invalid)?;
    let expires = if expires < 100000000000 {
        expires.checked_mul(1000).ok_or(Failure::Invalid)?
    } else {
        expires
    };
    let device = Device {
        auth_host: host,
        uid: uid.clone(),
        id: devices[0].1.into(),
        machine: value(&doc, "telemetry.machineId", 256)?,
        client: "ono9krqynydwx5".into(),
        version: value(&product, "appVersion", 128)?,
        version_code: value(&doc, "iCubeLastVersion", 128)?,
        os: os.into(),
        os_version: os_version.into(),
        source_digest: hex::encode(Sha256::digest(storage)),
        private_key,
        public_key,
    };
    let grant = Grant {
        access: auth
            .get("token")
            .or_else(|| auth.get("accessToken"))
            .and_then(Value::as_str)
            .filter(|s| store::text(s, 16384))
            .ok_or(Failure::Invalid)?
            .into(),
        refresh: value(&auth, "refreshToken", 16384)?,
        expires,
    };
    Ok(Material {
        account: Account {
            id: String::new(),
            uid,
            realm,
            generation: 1,
            version: 1,
            device,
            imported_refresh_hash: hex::encode(Sha256::digest(grant.refresh.as_bytes())),
            grant: Some(grant),
            refresh_pending: false,
            checkin_day: None,
        },
    })
}
#[cfg(test)]
pub(super) fn synthetic_material() -> Material {
    let (s, p) = tests::input();
    parse(&s, &p, Edition::Cn, "linux", "Linux synthetic").unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::json;
    pub(super) fn input() -> (Vec<u8>, Vec<u8>) {
        let rng = ring::rand::SystemRandom::new();
        let key = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &rng,
        )
        .unwrap();
        let kp = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            key.as_ref(),
            &rng,
        )
        .unwrap();
        use ring::signature::KeyPair;
        let mut spki = hex::decode("3059301306072a8648ce3d020106082a8648ce3d030107034200").unwrap();
        spki.extend_from_slice(kp.public_key().as_ref());
        let pem = |name: &str, bytes: &[u8]| {
            format!(
                "-----BEGIN {name}-----\n{}\n-----END {name}-----",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )
        };
        let doc = json!({"iCubeAuthInfo://icube.cloudide":json!({"token":"synthetic-access","refreshToken":"synthetic-refresh","userId":"synthetic-user","host":"https://api.trae.cn","userRegion":{"region":"CN"},"expiredAt":4102444800000_i64}).to_string(),"iCubeAuthInfo://icube-dc:1234567890123456":json!({"privateKeyPEM":pem("PRIVATE KEY",key.as_ref()),"publicKeyPEM":pem("PUBLIC KEY",&spki)}).to_string(),"telemetry.machineId":"f".repeat(64),"iCubeLastVersion":"20260917"});
        (
            serde_json::to_vec(&doc).unwrap(),
            serde_json::to_vec(&json!({"appVersion":"3.4.2"})).unwrap(),
        )
    }
    #[test]
    fn complete_same_storage_device_import_retains_original_ids() {
        let (s, p) = input();
        let m = parse(&s, &p, Edition::Cn, "linux", "Linux test").unwrap();
        assert_eq!(m.account.uid, "synthetic-user");
        assert_eq!(m.account.device.id, "1234567890123456");
        assert_eq!(m.account.device.machine, "f".repeat(64));
    }
    #[test]
    fn missing_or_ambiguous_device_rejects_instead_of_fabricating() {
        let (s, p) = input();
        let mut d: serde_json::Value = serde_json::from_slice(&s).unwrap();
        d.as_object_mut()
            .unwrap()
            .remove("iCubeAuthInfo://icube-dc:1234567890123456");
        assert!(parse(
            &serde_json::to_vec(&d).unwrap(),
            &p,
            Edition::Cn,
            "linux",
            "Linux"
        )
        .is_err());
    }
    #[test]
    fn proof_uses_same_registered_key_and_exact_newline_contract() {
        let (s, p) = input();
        let m = parse(&s, &p, Edition::Cn, "linux", "Linux").unwrap();
        let proof = super::super::auth::proof(
            &m.account.device,
            "synthetic-refresh",
            1792345200,
            "00112233445566778899aabbccddeeff",
        )
        .unwrap();
        let signature = base64::engine::general_purpose::STANDARD
            .decode(proof["Signature"].as_str().unwrap())
            .unwrap();
        let public = super::pem(&m.account.device.public_key, "PUBLIC KEY").unwrap();
        let message="POST\n/trae/api/v3/oauth/ExchangeToken\nono9krqynydwx5\nsynthetic-refresh\n1792345200\n00112233445566778899aabbccddeeff";
        let key = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_ASN1,
            &public[public.len() - 65..],
        );
        key.verify(message.as_bytes(), &signature).unwrap();
        assert!(key
            .verify(message.replace('\n', " ").as_bytes(), &signature)
            .is_err());
    }

    #[test]
    fn authentic_codec_golden_and_corruption_fail_closed() {
        let encoded="dGMFEAAAAAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh9zyayC5HBKM4FgOYOrddnw9iw4Sn0ZkPI86S3H2wCiaVqprGgeeodEULOWImBwEkKtlUMHCRVKSxTyROFWrTz4RsqFBv3evc8tQ+LJGd2JWbKVcK8eIZc4RHm/cgInagY=";
        assert_eq!(
            local_value(&serde_json::json!(encoded)).unwrap()["synthetic"],
            "offline-only"
        );
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        raw[70] ^= 1;
        assert!(local_value(&serde_json::json!(
            base64::engine::general_purpose::STANDARD.encode(raw)
        ))
        .is_err());
    }
    #[test]
    fn fresh_profile_global_import_requires_exact_desktop_host_and_package() {
        let (storage, _) = input();
        let mut doc: Value = serde_json::from_slice(&storage).unwrap();
        let mut auth: Value =
            serde_json::from_str(doc["iCubeAuthInfo://icube.cloudide"].as_str().unwrap()).unwrap();
        auth["host"] = serde_json::json!("https://growsg-normal.trae.ai");
        auth["userRegion"] = serde_json::json!({"region":"SG"});
        doc["iCubeAuthInfo://icube.cloudide"] = serde_json::json!(auth.to_string());
        let product = serde_json::json!({"appVersion":"3.6.2","packageType":"TRAE_I18N"});
        let bytes = serde_json::to_vec(&doc).unwrap();
        let good = parse(
            &bytes,
            &serde_json::to_vec(&product).unwrap(),
            Edition::Global,
            "linux",
            "Linux synthetic",
        )
        .unwrap();
        assert_eq!(good.account.realm, Realm::Sg);
        assert_eq!(good.account.device.version, "3.6.2");
        assert_eq!(
            good.account.device.auth_host,
            "https://growsg-normal.trae.ai"
        );
        auth["userRegion"] = serde_json::json!({"region":"US"});
        doc["iCubeAuthInfo://icube.cloudide"] = serde_json::json!(auth.to_string());
        let us = parse(
            &serde_json::to_vec(&doc).unwrap(),
            &serde_json::to_vec(&product).unwrap(),
            Edition::Global,
            "linux",
            "Linux synthetic",
        )
        .unwrap();
        assert_eq!(us.account.realm, Realm::Us);
        auth["userRegion"] = serde_json::json!({"region":"SG"});
        for package in [None, Some("TRAE_CN"), Some("TRAE_SOLO_I18N")] {
            let mut changed = product.clone();
            match package {
                Some(value) => changed["packageType"] = serde_json::json!(value),
                None => {
                    changed.as_object_mut().unwrap().remove("packageType");
                }
            }
            assert!(parse(
                &bytes,
                &serde_json::to_vec(&changed).unwrap(),
                Edition::Global,
                "linux",
                "Linux synthetic"
            )
            .is_err());
        }
        for host in [
            "https://api-sg-central.trae.ai",
            "https://api-us-east.trae.ai",
            "https://growsg-normal.trae.ai.unrelated.invalid",
        ] {
            auth["host"] = serde_json::json!(host);
            doc["iCubeAuthInfo://icube.cloudide"] = serde_json::json!(auth.to_string());
            assert!(parse(
                &serde_json::to_vec(&doc).unwrap(),
                &serde_json::to_vec(&product).unwrap(),
                Edition::Global,
                "linux",
                "Linux synthetic"
            )
            .is_err());
        }
    }
}
