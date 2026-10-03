use super::super::core::{AccountIdentity, OAuthFamily};
use super::*;
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
    encrypt_synthetic_with_secret(value, TEST_SECRET)
}
fn encrypt_synthetic_with_secret(value: &str, secret: &str) -> String {
    // Only synthetic tests create native ciphertext; there is no production encrypt API.
    static NONCE: AtomicU64 = AtomicU64::new(100);
    let mut iv = [0u8; 12];
    iv[4..].copy_from_slice(&NONCE.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    let cipher = Aes256Gcm::new_from_slice(&Sha256::digest(secret.as_bytes())).unwrap();
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
    native_document_with_context(TEST_CONTEXT, TEST_SECRET, family, id, version)
}
pub(crate) fn native_document_with_context(
    context: &str,
    secret: &str,
    family: OAuthFamily,
    id: &str,
    version: &str,
) -> CredentialDocument {
    let provider = match family {
        OAuthFamily::Zai => "zai",
        OAuthFamily::BigModel => "bigmodel",
    };
    let identity = AccountIdentity::new(context, family, id).unwrap();
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
        data.insert(key, encrypt_synthetic_with_secret(&plaintext, secret));
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

mod profile_labels {
    use super::*;

    fn snapshot(
        native: &NativeCipher,
        family: OAuthFamily,
        profile: serde_json::Value,
    ) -> AccountSnapshot {
        let document = native_document(family, "account-a", "fresh");
        let key = AccountIdentity::new(TEST_CONTEXT, family, "account-a")
            .unwrap()
            .credential_keys()[3]
            .clone();
        native
            .inspect(&replace(
                &document,
                &key,
                encrypt_synthetic(&profile.to_string()),
            ))
            .unwrap()
    }

    #[test]
    fn profile_label_prefers_display_name_for_both_standard_families() {
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
            let saved = snapshot(
                &native,
                family,
                serde_json::json!({
                    "id":"account-a", "username":"fallback", "displayName":" Display name "
                }),
            );
            assert_eq!(
                native.profile_label(&saved).unwrap().as_deref(),
                Some("Display name")
            );
        }
    }

    #[test]
    fn profile_label_reads_only_existing_raw_zai_display_fields() {
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        for (profile, expected) in [
            (
                serde_json::json!({"user_id":"account-a", "displayName":"Raw display", "username":"raw-user"}),
                Some("Raw display"),
            ),
            (
                serde_json::json!({"user_id":"account-a", "username":"raw-user"}),
                Some("raw-user"),
            ),
            (
                serde_json::json!({"user_id":"account-a", "name":"SYNTHETIC_NAME_CANARY", "email":"synthetic@example.invalid"}),
                None,
            ),
        ] {
            let saved = snapshot(&native, OAuthFamily::Zai, profile);
            assert_eq!(native.profile_label(&saved).unwrap().as_deref(), expected);
        }
    }

    #[test]
    fn profile_label_removes_controls_and_uses_nonempty_username_fallback() {
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        for (display, username, expected) in [
            (
                " \u{feff}\nDi\0sp\u{0085}lay\t ",
                "fallback",
                Some("Display"),
            ),
            (" \n\0\u{0085} ", " \nUser\0name\t ", Some("Username")),
            (" \n\0 ", " \t\u{0085} ", None),
        ] {
            let saved = snapshot(
                &native,
                OAuthFamily::BigModel,
                serde_json::json!({
                    "id":"account-a", "username":username, "displayName":display
                }),
            );
            assert_eq!(native.profile_label(&saved).unwrap().as_deref(), expected);
        }
    }

    #[test]
    fn profile_label_caps_unicode_scalars_without_breaking_utf8() {
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        let long = format!(" \n{}tail\t ", "中😀".repeat(50));
        let saved = snapshot(
            &native,
            OAuthFamily::Zai,
            serde_json::json!({
                "id":"account-a", "username":"fallback", "displayName":long
            }),
        );
        let label = native.profile_label(&saved).unwrap().unwrap();
        assert_eq!(label, "中😀".repeat(40));
        assert_eq!(label.chars().count(), 80);
    }

    #[test]
    fn profile_label_never_uses_unknown_token_fields_or_nested_labels() {
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        for display in [serde_json::Value::Null, serde_json::json!("Safe label")] {
            let expected = display.as_str().map(str::to_owned);
            let saved = snapshot(
                &native,
                OAuthFamily::Zai,
                serde_json::json!({
                    "user_id":"account-a", "displayName":display,
                    "username":{"displayName":"SYNTHETIC_NESTED_CANARY"},
                    "access_token":"SYNTHETIC_ACCESS_CANARY", "refresh_token":"SYNTHETIC_REFRESH_CANARY",
                    "name":"SYNTHETIC_NAME_CANARY", "future":{"username":"SYNTHETIC_FUTURE_CANARY"}
                }),
            );
            let label = native.profile_label(&saved).unwrap();
            assert_eq!(label, expected);
            assert!(!label.unwrap_or_default().contains("CANARY"));
        }
    }

    #[test]
    fn profile_label_reauthenticates_every_present_scoped_credential() {
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        let identity = AccountIdentity::new(TEST_CONTEXT, OAuthFamily::Zai, "account-a").unwrap();
        let document = native_document(OAuthFamily::Zai, "account-a", "fresh");
        for key in identity.credential_keys() {
            let changed = replace(
                &document,
                &key,
                encrypt_synthetic_with_secret("synthetic-tamper", "wrong-secret"),
            );
            let saved = AccountSnapshot::capture(identity.clone(), &changed).unwrap();
            assert_eq!(
                native.profile_label(&saved),
                Err(NativeError::AuthenticationFailed)
            );
        }
    }

    #[test]
    fn profile_label_rejects_snapshot_identity_or_context_substitution() {
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        let document = native_document(OAuthFamily::Zai, "account-a", "fresh");
        for identity in [
            AccountIdentity::new(TEST_CONTEXT, OAuthFamily::Zai, "other-account").unwrap(),
            AccountIdentity::new("other-context", OAuthFamily::Zai, "account-a").unwrap(),
        ] {
            let saved = AccountSnapshot::capture(identity, &document).unwrap();
            assert_eq!(
                native.profile_label(&saved),
                Err(NativeError::InvalidSession)
            );
        }
    }
}
