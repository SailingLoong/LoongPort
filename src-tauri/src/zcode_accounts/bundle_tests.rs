use super::super::{
    core::{AccountIdentity, CredentialDocument, OAuthFamily},
    native::{NativeCipher, NativeError},
};
use super::*;

const PASSWORD: &str = "synthetic-zsb-password";
const LOCAL_SECRET: &str = "zcode-credential-fallback:darwin:/synthetic/local:fixture-user";
const VALID: &[u8] =
    include_bytes!("../../../tests/zcode-bundle-limits/fixtures/valid-multiple.zsb");

#[test]
fn export_selected_snapshot_roundtrips_complete_credentials_and_official_name_only() {
    use super::super::native::tests::{native_document, TEST_CONTEXT, TEST_SECRET};

    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    let first = native
        .inspect(&native_document(
            OAuthFamily::BigModel,
            "synthetic-one",
            "one",
        ))
        .unwrap();
    let selected = native
        .inspect(&native_document(OAuthFamily::Zai, "synthetic-two", "two"))
        .unwrap();
    let file = encode_bundle(
        &[BundleExportAccount {
            snapshot: &selected,
            created_at: "2026-10-05T00:00:00Z",
        }],
        &native,
        PASSWORD,
        "2026-10-06T00:00:00Z",
    )
    .unwrap();
    let opened = open_bundle(&file, PASSWORD).unwrap();
    let entries = opened.entries().unwrap();
    assert_eq!(entries.len(), 1);
    let document =
        CredentialDocument::parse(entries[0].as_ref().unwrap().credentials.get().as_bytes())
            .unwrap();
    assert!(document == selected.scoped_document());
    assert!(native.inspect(&document).unwrap().identity() == selected.identity());
    assert!(native.inspect(&document).unwrap().identity() != first.identity());
    let inner: serde_json::Value = serde_json::from_slice(&opened.plaintext).unwrap();
    assert_eq!(inner["format"], "zcode-accounts-bundle");
    assert_eq!(inner["version"], 2);
    assert_eq!(inner["exportedAt"], "2026-10-06T00:00:00Z");
    let account = &inner["accounts"][0];
    assert_eq!(account["name"], "synthetic-two");
    assert_eq!(account["createdAt"], "2026-10-05T00:00:00Z");
    assert!(account["config"].is_null());
    assert_eq!(
        account
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["config", "createdAt", "credentials", "name"]
    );
    assert_eq!(account["credentials"].as_object().unwrap().len(), 7);
    assert!(account["credentials"].get("ssh:unrelated").is_none());
    let ciphertext = String::from_utf8(file).unwrap();
    for excluded in [
        "synthetic-one",
        "synthetic-two",
        "SYNTHETIC_CANARY",
        PASSWORD,
        "enc:v1:",
    ] {
        assert!(!ciphertext.contains(excluded));
    }
}

fn export_fixture() -> (NativeCipher, AccountSnapshot) {
    let native = NativeCipher::new("synthetic-target-context", LOCAL_SECRET).unwrap();
    let opened = open_bundle(VALID, PASSWORD).unwrap();
    let entries = opened.entries().unwrap();
    let document =
        CredentialDocument::parse(entries[0].as_ref().unwrap().credentials.get().as_bytes())
            .unwrap();
    let snapshot = native.inspect(&document).unwrap();
    (native, snapshot)
}

fn export_one(native: &NativeCipher, snapshot: &AccountSnapshot, password: &str) -> Vec<u8> {
    encode_bundle(
        &[BundleExportAccount {
            snapshot,
            created_at: "2026-10-05T00:00:00Z",
        }],
        native,
        password,
        "2026-10-06T00:00:00Z",
    )
    .unwrap()
}

#[test]
fn export_password_bytes_are_preserved_and_random_salt_and_nonce_change_each_time() {
    let (native, snapshot) = export_fixture();
    let password = " \t synthetic-口令-🔑 \n";
    let first = export_one(&native, &snapshot, password);
    let second = export_one(&native, &snapshot, password);
    assert!(open_bundle(&first, password).is_ok());
    assert_eq!(
        open_bundle(&first, password.trim()).err(),
        Some(BundleFailure::Authentication)
    );
    assert_eq!(
        open_bundle(&first, "wrong-synthetic-password").err(),
        Some(BundleFailure::Authentication)
    );
    let a: Envelope = serde_json::from_slice(&first).unwrap();
    let b: Envelope = serde_json::from_slice(&second).unwrap();
    assert_ne!(a.kdf.salt, b.kdf.salt);
    assert_ne!(a.cipher.nonce, b.cipher.nonce);
    assert_ne!(a.cipher.data, b.cipher.data);
    assert_eq!(STANDARD.decode(a.kdf.salt).unwrap().len(), 16);
    assert_eq!(STANDARD.decode(a.cipher.nonce).unwrap().len(), 12);
    assert_eq!(STANDARD.decode(a.cipher.tag).unwrap().len(), 16);
}

#[test]
fn export_rejects_empty_or_oversized_passwords_with_static_errors() {
    let (native, snapshot) = export_fixture();
    for password in [String::new(), " \r\n\t".into(), "a".repeat(4097)] {
        let error = encode_bundle(
            &[BundleExportAccount {
                snapshot: &snapshot,
                created_at: "synthetic",
            }],
            &native,
            &password,
            "synthetic",
        )
        .unwrap_err();
        assert_eq!(error, BundleFailure::Password);
        assert_eq!(format!("{error:?}"), "Password");
    }
    let maximum = "x".repeat(4096);
    assert!(open_bundle(&export_one(&native, &snapshot, &maximum), &maximum).is_ok());
}

#[test]
fn export_requires_every_selected_snapshot_to_authenticate_in_the_same_native_context() {
    let (native, snapshot) = export_fixture();
    let foreign_context = NativeCipher::new("other-synthetic-context", LOCAL_SECRET).unwrap();
    let foreign_key =
        NativeCipher::new("synthetic-target-context", "foreign-synthetic-secret").unwrap();
    for cipher in [&foreign_context, &foreign_key] {
        let error = encode_bundle(
            &[BundleExportAccount {
                snapshot: &snapshot,
                created_at: "synthetic",
            }],
            cipher,
            PASSWORD,
            "synthetic",
        )
        .unwrap_err();
        assert_eq!(error, BundleFailure::Account);
        assert_eq!(format!("{error:?}"), "Account");
    }
    let forged = AccountSnapshot::capture(
        AccountIdentity::new(
            "synthetic-target-context",
            OAuthFamily::BigModel,
            "forged-synthetic-id",
        )
        .unwrap(),
        &snapshot.scoped_document(),
    )
    .unwrap();
    assert_eq!(
        encode_bundle(
            &[
                BundleExportAccount {
                    snapshot: &snapshot,
                    created_at: "synthetic"
                },
                BundleExportAccount {
                    snapshot: &forged,
                    created_at: "synthetic"
                },
            ],
            &native,
            PASSWORD,
            "synthetic",
        )
        .err(),
        Some(BundleFailure::Account)
    );
}

#[test]
fn export_enforces_account_limit_and_accepts_fifty_complete_snapshots() {
    use super::super::bundle_limits::MAX_BUNDLE_ACCOUNTS;
    let (native, snapshot) = export_fixture();
    for count in [0, MAX_BUNDLE_ACCOUNTS + 1] {
        let accounts = (0..count)
            .map(|_| BundleExportAccount {
                snapshot: &snapshot,
                created_at: "synthetic",
            })
            .collect::<Vec<_>>();
        assert_eq!(
            encode_bundle(&accounts, &native, PASSWORD, "synthetic").err(),
            Some(BundleFailure::Limits(BundleError::ResourceLimit))
        );
    }
    let accounts = (0..MAX_BUNDLE_ACCOUNTS)
        .map(|_| BundleExportAccount {
            snapshot: &snapshot,
            created_at: "synthetic",
        })
        .collect::<Vec<_>>();
    let file = encode_bundle(&accounts, &native, PASSWORD, "synthetic").unwrap();
    assert_eq!(
        open_bundle(&file, PASSWORD)
            .unwrap()
            .entries()
            .unwrap()
            .len(),
        MAX_BUNDLE_ACCOUNTS
    );
}

#[test]
fn export_enforces_inner_and_encoded_file_size_limits_including_json_escaping() {
    let (native, snapshot) = export_fixture();
    let accounts = [BundleExportAccount {
        snapshot: &snapshot,
        created_at: "synthetic",
    }];
    // The first inner JSON fits 10 MiB, but its base64 envelope cannot fit.
    for timestamp in [
        "x".repeat(8 * 1024 * 1024),
        "x".repeat(MAX_BUNDLE_BYTES),
        "\"".repeat(6 * 1024 * 1024),
    ] {
        assert_eq!(
            encode_bundle(&accounts, &native, PASSWORD, &timestamp).err(),
            Some(BundleFailure::Limits(BundleError::ResourceLimit))
        );
    }
}

#[test]
fn export_ciphertext_is_decryptable_by_independent_ring_aead_and_detects_tampering() {
    let (native, snapshot) = export_fixture();
    let file = export_one(&native, &snapshot, PASSWORD);
    let mut envelope: serde_json::Value = serde_json::from_slice(&file).unwrap();
    assert_eq!(envelope["format"], "zsw-accounts-bundle");
    assert_eq!(envelope["version"], 1);
    assert_eq!(envelope["kdf"]["algo"], "pbkdf2-hmac-sha256");
    assert_eq!(envelope["kdf"]["iters"], 100_000);
    assert_eq!(envelope["cipher"]["algo"], "aes-256-gcm");
    let salt = STANDARD
        .decode(envelope["kdf"]["salt"].as_str().unwrap())
        .unwrap();
    let nonce: [u8; 12] = STANDARD
        .decode(envelope["cipher"]["nonce"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let tag = STANDARD
        .decode(envelope["cipher"]["tag"].as_str().unwrap())
        .unwrap();
    let mut encrypted = STANDARD
        .decode(envelope["cipher"]["data"].as_str().unwrap())
        .unwrap();
    encrypted.extend_from_slice(&tag);
    let mut key = Zeroizing::new([0u8; 32]);
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(100_000).unwrap(),
        &salt,
        PASSWORD.as_bytes(),
        &mut *key,
    );
    let cipher = ring::aead::LessSafeKey::new(
        ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, &*key).unwrap(),
    );
    let mut clear = Zeroizing::new(encrypted);
    let plaintext = cipher
        .open_in_place(
            ring::aead::Nonce::assume_unique_for_key(nonce),
            ring::aead::Aad::empty(),
            &mut clear,
        )
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(plaintext).unwrap();
    assert_eq!(payload["accounts"][0]["name"], "Synthetic account");
    assert!(payload["accounts"][0]["config"].is_null());
    let mut corrupted = tag;
    corrupted[0] ^= 1;
    envelope["cipher"]["tag"] = STANDARD.encode(corrupted).into();
    assert_eq!(
        open_bundle(&serde_json::to_vec(&envelope).unwrap(), PASSWORD).err(),
        Some(BundleFailure::Authentication)
    );
}

#[test]
fn fixed_multi_account_vector_authenticates_and_preserves_native_identities() {
    let payload = open_bundle(VALID, PASSWORD).unwrap();
    let entries = payload.entries().unwrap();
    assert_eq!(entries.len(), 2);
    let cipher = NativeCipher::new("synthetic-target-context", LOCAL_SECRET).unwrap();
    for (entry, id) in entries.iter().zip(["synthetic-one", "synthetic-two"]) {
        let raw = entry.as_ref().unwrap().credentials;
        let document = CredentialDocument::parse(raw.get().as_bytes()).unwrap();
        let snapshot = cipher.inspect(&document).unwrap();
        assert!(
            snapshot.identity()
                == &AccountIdentity::new("synthetic-target-context", OAuthFamily::BigModel, id)
                    .unwrap()
        );
    }
}

#[test]
fn wrong_password_and_damaged_tag_have_only_static_authentication_errors() {
    assert_eq!(
        open_bundle(VALID, "incorrect-synthetic-password").err(),
        Some(BundleFailure::Authentication)
    );
    let damaged = include_bytes!(
        "../../../tests/zcode-bundle-limits/fixtures/damaged-authentication-tag.zsb"
    );
    assert_eq!(
        open_bundle(damaged, PASSWORD).err(),
        Some(BundleFailure::Authentication)
    );
}

#[test]
fn duplicate_outer_keys_and_truncation_are_rejected() {
    for file in [
        include_bytes!("../../../tests/zcode-bundle-limits/fixtures/duplicate-outer-key.zsb")
            .as_slice(),
        include_bytes!("../../../tests/zcode-bundle-limits/fixtures/truncated.zsb").as_slice(),
    ] {
        assert_eq!(
            open_bundle(file, PASSWORD).err(),
            Some(BundleFailure::Envelope)
        );
    }
}

#[test]
fn unknown_version_and_algorithm_are_rejected_before_crypto() {
    assert_eq!(
        open_bundle(
            include_bytes!("../../../tests/zcode-bundle-limits/fixtures/unknown-outer-version.zsb"),
            PASSWORD
        )
        .err(),
        Some(BundleFailure::Limits(BundleError::UnsupportedVersion))
    );
    assert_eq!(
        open_bundle(
            include_bytes!("../../../tests/zcode-bundle-limits/fixtures/unknown-kdf.zsb"),
            PASSWORD
        )
        .err(),
        Some(BundleFailure::Limits(BundleError::InvalidKdf))
    );
}

#[test]
fn unexpected_inner_schema_and_excess_accounts_do_not_produce_candidates() {
    assert_eq!(
        open_bundle(
            include_bytes!(
                "../../../tests/zcode-bundle-limits/fixtures/unexpected-inner-field.zsb"
            ),
            PASSWORD
        )
        .err(),
        Some(BundleFailure::Inner)
    );
    assert_eq!(
        open_bundle(
            include_bytes!("../../../tests/zcode-bundle-limits/fixtures/over-account-limit.zsb"),
            PASSWORD
        )
        .err(),
        Some(BundleFailure::Limits(BundleError::ResourceLimit))
    );
}

#[test]
fn foreign_inner_key_is_not_mistaken_for_outer_authentication_failure() {
    let payload = open_bundle(
        include_bytes!("../../../tests/zcode-bundle-limits/fixtures/foreign-inner-key.zsb"),
        PASSWORD,
    )
    .unwrap();
    let entries = payload.entries().unwrap();
    let document =
        CredentialDocument::parse(entries[0].as_ref().unwrap().credentials.get().as_bytes())
            .unwrap();
    let cipher = NativeCipher::new("synthetic-target-context", LOCAL_SECRET).unwrap();
    assert_eq!(
        cipher.inspect(&document).err(),
        Some(NativeError::AuthenticationFailed)
    );
}

#[test]
fn different_ciphertext_for_same_identity_is_not_a_distinct_account() {
    let payload = open_bundle(
        include_bytes!("../../../tests/zcode-bundle-limits/fixtures/duplicate-identity.zsb"),
        PASSWORD,
    )
    .unwrap();
    let entries = payload.entries().unwrap();
    let cipher = NativeCipher::new("synthetic-target-context", LOCAL_SECRET).unwrap();
    let identities: Vec<_> = entries
        .iter()
        .map(|entry| {
            cipher
                .inspect(
                    &CredentialDocument::parse(
                        entry.as_ref().unwrap().credentials.get().as_bytes(),
                    )
                    .unwrap(),
                )
                .unwrap()
        })
        .collect();
    assert!(identities[0].identity() == identities[1].identity());
    assert_ne!(
        entries[0].as_ref().unwrap().credentials.get(),
        entries[1].as_ref().unwrap().credentials.get()
    );
}

#[test]
fn malformed_account_is_an_unselectable_item_and_config_is_never_returned() {
    let raw = br#"{"format":"zcode-accounts-bundle","version":2,"exportedAt":"synthetic","accounts":[{"name":"x","createdAt":"synthetic","credentials":{"key":"encrypted"},"config":{"device":"do-not-import"}},{"name":"bad","createdAt":"synthetic","credentials":[],"config":null}]}"#;
    let entries = inspect_inner(raw).unwrap();
    assert!(entries[0].is_ok());
    assert_eq!(entries[1].as_ref().err(), Some(&EntryFailure::Shape));
    assert!(!entries[0]
        .as_ref()
        .unwrap()
        .credentials
        .get()
        .contains("do-not-import"));
}

#[test]
fn duplicate_credential_keys_are_not_silently_overwritten() {
    let raw = br#"{"format":"zcode-accounts-bundle","version":2,"exportedAt":"synthetic","accounts":[{"name":"x","createdAt":"synthetic","credentials":{"key":"one","key":"two"},"config":null}]}"#;
    let entries = inspect_inner(raw).unwrap();
    assert_eq!(entries[0].as_ref().err(), Some(&EntryFailure::Shape));
}

#[test]
fn package_password_and_file_bounds_are_enforced_before_parse_or_kdf() {
    assert_eq!(
        open_bundle(VALID, "  ").err(),
        Some(BundleFailure::Password)
    );
    assert_eq!(
        open_bundle(&vec![0; MAX_BUNDLE_BYTES + 1], PASSWORD).err(),
        Some(BundleFailure::Limits(BundleError::ResourceLimit))
    );
}
