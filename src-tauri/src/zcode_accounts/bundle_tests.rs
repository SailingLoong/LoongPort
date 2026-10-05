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
