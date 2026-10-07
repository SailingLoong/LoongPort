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
