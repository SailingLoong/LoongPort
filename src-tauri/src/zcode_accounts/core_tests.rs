use super::*;

mod document_and_scope {
    use super::*;

    #[test]
    fn preserves_unknown_credentials_as_opaque_strings() {
        let doc = CredentialDocument::parse(br#"{"ssh:example":"opaque","future:kind":"enc:v99:keep","oauth:login_attribution":"unchanged"}"#).unwrap();
        assert_eq!(doc.get("ssh:example"), Some("opaque"));
        assert_eq!(doc.get("future:kind"), Some("enc:v99:keep"));
        assert_eq!(doc.get("oauth:login_attribution"), Some("unchanged"));
    }

    #[test]
    fn rejects_duplicate_keys_including_escaped_aliases() {
        for raw in [
            br#"{"a":"one","a":"two"}"#.as_slice(),
            br#"{"a":"one","\u0061":"two"}"#,
        ] {
            assert!(matches!(
                CredentialDocument::parse(raw),
                Err(CoreError::InvalidDocument)
            ));
        }
    }

    #[test]
    fn rejects_non_string_values_invalid_utf8_and_non_objects() {
        for raw in [
            br#"{"a":null}"#.as_slice(),
            br#"{"a":3}"#,
            br#"[]"#,
            b"{",
            b"\xff",
        ] {
            assert!(matches!(
                CredentialDocument::parse(raw),
                Err(CoreError::InvalidDocument)
            ));
        }
    }

    #[test]
    fn accepts_empty_document_but_caps_input_before_parsing() {
        assert!(CredentialDocument::parse(b"{}").is_ok());
        let mut exact = vec![b' '; MAX_DOCUMENT_BYTES];
        exact[..2].copy_from_slice(b"{}");
        assert!(CredentialDocument::parse(&exact).is_ok());
        exact.push(b' ');
        assert!(matches!(
            CredentialDocument::parse(&exact),
            Err(CoreError::DocumentTooLarge)
        ));
    }

    #[test]
    fn contains_only_the_seven_exact_keys_for_each_family() {
        for (family, provider) in [
            (OAuthFamily::Zai, "zai"),
            (OAuthFamily::BigModel, "bigmodel"),
        ] {
            let id = AccountIdentity::new("synthetic-context", family, "account-a").unwrap();
            let expected = vec![
                "oauth:active_provider".to_string(),
                format!("oauth:{provider}:access_token"),
                format!("oauth:{provider}:refresh_token"),
                format!("oauth:{provider}:user_info"),
                "zcodejwttoken".to_string(),
                format!("account-provider:coding-plan:account:{provider}-individual-coding-plan:account:account-a:api-key"),
                format!("account-provider:start-plan:account:{provider}-start-plan:account:account-a:api-key"),
            ];
            assert_eq!(id.credential_keys(), expected);
        }
    }

    #[test]
    fn matches_ecmascript_trim_and_encode_uri_component() {
        let vectors = [
            (
                " \u{feff}a:b/% !'()*-._~中😀\u{3000}",
                "a%3Ab%2F%25%20!'()*-._~%E4%B8%AD%F0%9F%98%80",
            ),
            ("\u{85}id\u{85}", "%C2%85id%C2%85"),
            ("\u{200b}id\u{200b}", "%E2%80%8Bid%E2%80%8B"),
        ];
        for (raw, encoded) in vectors {
            let id = AccountIdentity::new("synthetic-context", OAuthFamily::Zai, raw).unwrap();
            assert_eq!(id.credential_keys()[5], format!("account-provider:coding-plan:account:zai-individual-coding-plan:account:{encoded}:api-key"));
        }
    }

    #[test]
    fn rejects_absent_and_unknown_identities_without_echoing_inputs() {
        for id in ["", " \u{feff}\u{3000}", "unknown"] {
            assert!(matches!(
                AccountIdentity::new("context", OAuthFamily::Zai, id),
                Err(CoreError::InvalidIdentity)
            ));
        }
        assert!(matches!(
            AccountIdentity::new(" ", OAuthFamily::Zai, "a"),
            Err(CoreError::InvalidIdentity)
        ));
        let error = AccountIdentity::new("context", OAuthFamily::Zai, "unknown")
            .err()
            .unwrap();
        assert_eq!(format!("{error:?}"), "InvalidIdentity");
    }
}
