use super::*;
use crate::core::{AccountIdentity, OAuthFamily};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const TEST_SECRET: &str = "test-native-secret";
pub(crate) const TEST_CONTEXT: &str = "synthetic-native-context";

pub(crate) fn encrypt_synthetic(value: &str) -> String {
    // Only synthetic tests create native ciphertext; there is no production encrypt API.
    static NONCE: AtomicU64 = AtomicU64::new(100);
    let mut iv = [0u8; 12];
    iv[4..].copy_from_slice(&NONCE.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    let cipher = Aes256Gcm::new_from_slice(&Sha256::digest(TEST_SECRET.as_bytes())).unwrap();
    let mut encrypted = cipher.encrypt(&Nonce::from(iv), value.as_bytes()).unwrap();
    let tag = encrypted.split_off(encrypted.len() - 16);
    format!(
        "enc:v1:{}.{}.{}",
        URL_SAFE_NO_PAD.encode(iv),
        URL_SAFE_NO_PAD.encode(tag),
        URL_SAFE_NO_PAD.encode(encrypted)
    )
}

pub(crate) fn native_document(family: OAuthFamily, id: &str, version: &str) -> CredentialDocument {
    let provider = match family {
        OAuthFamily::Zai => "zai",
        OAuthFamily::BigModel => "bigmodel",
    };
    let identity = AccountIdentity::new(TEST_CONTEXT, family, id).unwrap();
    let mut data = BTreeMap::new();
    for key in identity.credential_keys() {
        let plaintext = if key == "oauth:active_provider" {
            provider.to_owned()
        } else if key.ends_with(":user_info") {
            serde_json::json!({"id":id,"username":id,"displayName":id,"future":{"keep":"nested"}})
                .to_string()
        } else {
            format!("SYNTHETIC_CANARY_{id}_{version}_{key}")
        };
        data.insert(key, encrypt_synthetic(&plaintext));
    }
    data.insert("ssh:unrelated".into(), "unrelated-opaque-format".into());
    CredentialDocument::parse(&serde_json::to_vec(&data).unwrap()).unwrap()
}

pub(crate) fn replace(
    document: &CredentialDocument,
    key: &str,
    value: String,
) -> CredentialDocument {
    let mut data: BTreeMap<String, String> =
        serde_json::from_slice(&document.to_bytes().unwrap()).unwrap();
    data.insert(key.into(), value);
    CredentialDocument::parse(&serde_json::to_vec(&data).unwrap()).unwrap()
}

#[test]
fn native_decrypts_independent_node_golden_vector() {
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    let raw = "enc:v1:AAECAwQFBgcICQoL.18eLOjNkonAZdSGQQ1xwQw.VjryTr6DjWO_8SN_N50f_-VZPSqZEg";
    assert_eq!(native.decrypt(raw).unwrap().as_str(), "native-canary-中-😀");
}

#[test]
fn native_rejects_missing_explicit_context_or_secret() {
    assert!(matches!(
        NativeCipher::new(" ", TEST_SECRET),
        Err(NativeError::MissingContext)
    ));
    assert!(matches!(
        NativeCipher::new(TEST_CONTEXT, ""),
        Err(NativeError::MissingSecret)
    ));
}

#[test]
fn native_rejects_unknown_plaintext_malformed_or_tampered_values() {
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    for raw in ["plain-secret", "enc:v2:a.b.c"] {
        assert!(matches!(
            native.decrypt(raw),
            Err(NativeError::UnsupportedCipher)
        ));
    }
    for raw in [
        "enc:v1:a.b.c",
        "enc:v1:a.b.c.d",
        "enc:v1:***.***.***",
        "enc:v1:..",
    ] {
        assert!(matches!(
            native.decrypt(raw),
            Err(NativeError::InvalidEnvelope)
        ));
    }
    let original = encrypt_synthetic("SYNTHETIC_CANARY");
    let wrong = NativeCipher::new(TEST_CONTEXT, "different-test-secret").unwrap();
    assert!(matches!(
        wrong.decrypt(&original),
        Err(NativeError::AuthenticationFailed)
    ));
    let mut parts: Vec<_> = original[7..].split('.').map(str::to_owned).collect();
    parts[1] = URL_SAFE_NO_PAD.encode([0u8; 16]);
    assert!(matches!(
        native.decrypt(&format!("enc:v1:{}", parts.join("."))),
        Err(NativeError::AuthenticationFailed)
    ));
}

#[test]
fn native_reads_standard_identity_for_both_families_without_decrypting_ssh() {
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let document = native_document(family, "a", "fresh");
        let snapshot = native.inspect(&document).unwrap();
        assert!(snapshot.identity() == &AccountIdentity::new(TEST_CONTEXT, family, "a").unwrap());
        assert_eq!(
            document.get("ssh:unrelated"),
            Some("unrelated-opaque-format")
        );
    }
}

#[test]
fn native_reads_zai_raw_profile_and_keeps_unknown_fields_opaque() {
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    let document = native_document(OAuthFamily::Zai, "a", "fresh");
    let raw =
        r#"{"user_id":"a","name":"A","email":"test@example.invalid","future":{"nested":true}}"#;
    let changed = replace(&document, "oauth:zai:user_info", encrypt_synthetic(raw));
    assert!(
        native.inspect(&changed).unwrap().identity()
            == &AccountIdentity::new(TEST_CONTEXT, OAuthFamily::Zai, "a").unwrap()
    );
}

#[test]
fn native_zai_raw_profile_allows_extra_standard_display_fields_without_an_id() {
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    let document = native_document(OAuthFamily::Zai, "a", "fresh");
    let raw = r#"{"user_id":"a","username":"a","displayName":"A"}"#;
    let changed = replace(&document, "oauth:zai:user_info", encrypt_synthetic(raw));
    assert!(native.inspect(&changed).is_ok());
}

#[test]
fn native_rejects_ambiguous_duplicate_empty_or_unsupported_identity_shapes() {
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    let document = native_document(OAuthFamily::Zai, "a", "fresh");
    for raw in [
        r#"{"id":"a","id":"b","username":"a","displayName":"a"}"#,
        r#"{"id":"a","user_id":"b","username":"a","displayName":"a"}"#,
        r#"{"user_id":"unknown","name":"A"}"#,
        r#"{"user_id":"","name":"A"}"#,
        r#"{"id":"a"}"#,
    ] {
        assert!(native
            .inspect(&replace(
                &document,
                "oauth:zai:user_info",
                encrypt_synthetic(raw)
            ))
            .is_err());
    }
    let bigmodel = native_document(OAuthFamily::BigModel, "a", "fresh");
    assert!(native
        .inspect(&replace(
            &bigmodel,
            "oauth:bigmodel:user_info",
            encrypt_synthetic(r#"{"user_id":"a","name":"A"}"#)
        ))
        .is_err());
    assert!(matches!(
        native.inspect(&replace(
            &document,
            "oauth:active_provider",
            encrypt_synthetic("unknown-provider")
        )),
        Err(NativeError::UnsupportedProvider)
    ));
}

#[test]
fn native_checks_optional_scoped_credentials_and_rejects_empty_secrets() {
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    let document = native_document(OAuthFamily::Zai, "a", "fresh");
    for key in [
        "oauth:zai:refresh_token",
        "zcodejwttoken",
        "account-provider:coding-plan:account:zai-individual-coding-plan:account:a:api-key",
    ] {
        assert!(native
            .inspect(&replace(&document, key, "enc:v99:unknown".into()))
            .is_err());
    }
    assert!(native
        .inspect(&replace(
            &document,
            "oauth:zai:access_token",
            encrypt_synthetic(" ")
        ))
        .is_err());
}
