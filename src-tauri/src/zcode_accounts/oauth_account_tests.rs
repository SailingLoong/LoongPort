use super::super::core::{AccountIdentity, OAuthFamily};
use super::super::official::{OfficialUser, ProviderAccessToken, SecretValue, StartJwt};
use super::*;

fn ready(family: OAuthFamily) -> PollReady {
    PollReady {
        family,
        user: OfficialUser {
            id: "synthetic/user + identity".into(),
            name: Some("Sample user".into()),
            email: Some("sample@example.invalid".into()),
        },
        start_jwt: StartJwt::new("synthetic-start-jwt").unwrap(),
        provider_access_token: ProviderAccessToken::new("synthetic-oauth-token").unwrap(),
        refresh_token: Some(SecretValue::new("synthetic-refresh").unwrap()),
    }
}

#[test]
fn official_image_stores_normalized_business_start_and_coding_credentials_separately() {
    for family in [OAuthFamily::BigModel, OAuthFamily::Zai] {
        let native = NativeCipher::new("synthetic-context", "synthetic-key-context").unwrap();
        let ready = ready(family);
        let business = BusinessToken::from_stored(family, "synthetic-business-token").unwrap();
        let coding = CodingKey::new("synthetic-coding-key").unwrap();
        let snapshot = build_snapshot(&native, &ready, &business, Some(&coding)).unwrap();
        assert!(
            snapshot.identity()
                == &AccountIdentity::new("synthetic-context", family, &ready.user.id).unwrap()
        );
        let keys = snapshot.identity().credential_keys();
        let image = snapshot.scoped_document();
        assert_eq!(
            native
                .decrypt(image.get(&keys[1]).unwrap())
                .unwrap()
                .as_str(),
            "synthetic-business-token"
        );
        assert_eq!(
            native
                .decrypt(image.get(&keys[2]).unwrap())
                .unwrap()
                .as_str(),
            "synthetic-refresh"
        );
        assert_eq!(
            native
                .decrypt(image.get(&keys[4]).unwrap())
                .unwrap()
                .as_str(),
            "synthetic-start-jwt"
        );
        assert_eq!(
            native
                .decrypt(image.get(&keys[5]).unwrap())
                .unwrap()
                .as_str(),
            "synthetic-coding-key"
        );
        assert_eq!(
            native
                .decrypt(image.get(&keys[6]).unwrap())
                .unwrap()
                .as_str(),
            "synthetic-start-jwt"
        );
        assert!(keys[5].contains("synthetic%2Fuser%20%2B%20identity"));
        assert!(native.inspect(&image).unwrap().identity() == snapshot.identity());
        let stored = String::from_utf8(image.to_bytes().unwrap()).unwrap();
        assert!(!stored.contains("synthetic-business-token"));
        assert!(!stored.contains("synthetic-oauth-token"));
    }
}

#[test]
fn declining_coding_key_keeps_login_image_without_inventing_a_key() {
    let native = NativeCipher::new("synthetic-context", "synthetic-key-context").unwrap();
    let ready = ready(OAuthFamily::Zai);
    let business =
        BusinessToken::from_stored(OAuthFamily::Zai, "synthetic-business-token").unwrap();
    let snapshot = build_snapshot(&native, &ready, &business, None).unwrap();
    let keys = snapshot.identity().credential_keys();
    assert!(snapshot.scoped_document().get(&keys[5]).is_none());
    assert!(snapshot.scoped_document().get(&keys[4]).is_some());
}

#[test]
fn provider_family_mismatch_cannot_build_a_mixed_image() {
    let native = NativeCipher::new("synthetic-context", "synthetic-key-context").unwrap();
    let ready = ready(OAuthFamily::Zai);
    let business =
        BusinessToken::from_stored(OAuthFamily::BigModel, "synthetic-business-token").unwrap();
    assert!(build_snapshot(&native, &ready, &business, None).is_err());
}

#[test]
fn saved_coding_addition_changes_only_the_selected_key_ciphertext() {
    let native = NativeCipher::new("synthetic-context", "synthetic-key-context").unwrap();
    let original = build_snapshot(
        &native,
        &ready(OAuthFamily::Zai),
        &BusinessToken::from_stored(OAuthFamily::Zai, "saved-business").unwrap(),
        None,
    )
    .unwrap();
    let coding = CodingKey::new("saved-coding").unwrap();
    let candidate = complete_coding_snapshot(&native, &original, Some(&coding)).unwrap();
    assert!(coding_only_change(&original, &candidate));
    assert!(original.identity() == candidate.identity());
    let before = original.scoped_document();
    let after = candidate.scoped_document();
    for (index, key) in original.identity().credential_keys().iter().enumerate() {
        if index != 5 {
            assert_eq!(
                before.get(key),
                after.get(key),
                "unchanged credential slot {index}"
            );
        }
    }
    assert_eq!(
        saved_business_token(&native, &candidate).unwrap().expose(),
        "saved-business"
    );
    assert_eq!(
        saved_coding_key(&native, &candidate).unwrap().expose(),
        "saved-coding"
    );
    assert!(saved_coding_key(&native, &original).is_none());
}

#[test]
fn saved_coding_decline_preserves_every_original_byte_and_rejects_other_session_fields() {
    let native = NativeCipher::new("synthetic-context", "synthetic-key-context").unwrap();
    let original = build_snapshot(
        &native,
        &ready(OAuthFamily::BigModel),
        &BusinessToken::from_stored(OAuthFamily::BigModel, "saved-business").unwrap(),
        None,
    )
    .unwrap();
    let same = complete_coding_snapshot(&native, &original, None).unwrap();
    assert_eq!(
        original.scoped_document().to_bytes().unwrap(),
        same.scoped_document().to_bytes().unwrap()
    );
    let other = build_snapshot(
        &native,
        &ready(OAuthFamily::BigModel),
        &BusinessToken::from_stored(OAuthFamily::BigModel, "other-business").unwrap(),
        Some(&CodingKey::new("coding").unwrap()),
    )
    .unwrap();
    assert!(!coding_only_change(&original, &other));
    assert!(saved_business_token(
        &NativeCipher::new("synthetic-context", "wrong-secret").unwrap(),
        &original
    )
    .is_err());
}

#[test]
fn saved_session_binding_ignores_encryption_nonce_but_rejects_changed_secrets() {
    let native = NativeCipher::new("synthetic-context", "synthetic-key-context").unwrap();
    let ready = ready(OAuthFamily::Zai);
    let business =
        BusinessToken::from_stored(OAuthFamily::Zai, "synthetic-business-token").unwrap();
    let original = build_snapshot(&native, &ready, &business, None).unwrap();
    let reencryption = build_snapshot(&native, &ready, &business, None).unwrap();
    assert_ne!(
        original.scoped_document().to_bytes().unwrap(),
        reencryption.scoped_document().to_bytes().unwrap()
    );
    assert_eq!(
        saved_session_binding(&native, &original).unwrap(),
        saved_session_binding(&native, &reencryption).unwrap()
    );
    let changed = build_snapshot(
        &native,
        &ready,
        &BusinessToken::from_stored(OAuthFamily::Zai, "different-business-token").unwrap(),
        None,
    )
    .unwrap();
    assert_ne!(
        saved_session_binding(&native, &original).unwrap(),
        saved_session_binding(&native, &changed).unwrap()
    );
}
