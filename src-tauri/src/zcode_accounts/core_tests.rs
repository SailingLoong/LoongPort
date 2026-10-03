use super::*;

fn account(family: OAuthFamily, id: &str) -> AccountIdentity {
    AccountIdentity::new("synthetic-context", family, id).unwrap()
}

fn session(identity: &AccountIdentity, version: &str) -> CredentialDocument {
    CredentialDocument(
        identity
            .credential_keys()
            .into_iter()
            .map(|key| {
                let value = format!("synthetic:{}:{version}:{key}", identity.account_id);
                (key, value)
            })
            .collect(),
    )
}

fn assert_same_document(actual: &CredentialDocument, expected: &CredentialDocument) {
    assert!(
        actual == expected,
        "credential document differs (values redacted)"
    );
}

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

mod switching {
    use super::*;

    #[test]
    fn serializes_without_losing_unknown_values() {
        let document =
            CredentialDocument::parse(br#"{"future":"\u0061","ssh:x":"preserve"}"#).unwrap();
        assert_same_document(
            &CredentialDocument::parse(&document.to_bytes().unwrap()).unwrap(),
            &document,
        );
    }

    #[test]
    fn merged_output_must_remain_within_the_read_limit() {
        let a = account(OAuthFamily::Zai, "a");
        let b = account(OAuthFamily::Zai, "b");
        let mut current = session(&a, "fresh");
        current.0.insert(
            "unrelated:large".into(),
            "x".repeat(MAX_DOCUMENT_BYTES / 2 + 10),
        );
        let mut target_document = session(&b, "fresh");
        target_document.0.insert(
            b.credential_keys()[1].clone(),
            "y".repeat(MAX_DOCUMENT_BYTES / 2 + 10),
        );
        let target = AccountSnapshot::capture(b, &target_document).unwrap();
        let plan = SwitchPlan::prepare(&current, &a, &target).unwrap();
        assert!(matches!(
            plan.apply(&current),
            Err(CoreError::DocumentTooLarge)
        ));
    }

    #[test]
    fn captures_a_prime_then_b_prime_and_returns_to_a_prime_for_both_families() {
        for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
            let a = account(family, "a");
            let b = account(family, "b");
            let a0 = AccountSnapshot::capture(a.clone(), &session(&a, "0")).unwrap();
            let b0 = AccountSnapshot::capture(b.clone(), &session(&b, "0")).unwrap();
            let a_prime = session(&a, "refreshed");
            let to_b = SwitchPlan::prepare(&a_prime, &a, &b0).unwrap();
            let on_b = to_b.apply(&a_prime).unwrap();
            assert!(!to_b.is_noop());
            assert_eq!(
                on_b.get(&b.credential_keys()[1]),
                session(&b, "0").get(&b.credential_keys()[1])
            );
            let b_prime = session(&b, "refreshed");
            let to_a = SwitchPlan::prepare(&b_prime, &b, to_b.fresh_source()).unwrap();
            let on_a = to_a.apply(&b_prime).unwrap();
            for key in a.credential_keys() {
                assert_eq!(on_a.get(&key), a_prime.get(&key));
            }
            assert_ne!(to_b.fresh_source().values, a0.values);
            assert_eq!(
                to_a.fresh_source().values,
                AccountSnapshot::capture(b, &b_prime).unwrap().values
            );
        }
    }

    #[test]
    fn selecting_a_stale_alias_of_current_identity_never_restores_a0() {
        let a = account(OAuthFamily::Zai, "a");
        let old = AccountSnapshot::capture(a.clone(), &session(&a, "old")).unwrap();
        let fresh = session(&a, "refreshed");
        let alias = AccountIdentity::new("synthetic-context", OAuthFamily::Zai, " a ").unwrap();
        let plan = SwitchPlan::prepare(&fresh, &alias, &old).unwrap();
        assert!(plan.is_noop());
        assert_same_document(&plan.apply(&fresh).unwrap(), &fresh);
        assert_eq!(
            plan.fresh_source().values,
            AccountSnapshot::capture(a, &fresh).unwrap().values
        );
    }

    #[test]
    fn keeps_other_credentials_and_deletes_only_absent_target_optional_keys() {
        let a = account(OAuthFamily::Zai, "a");
        let b = account(OAuthFamily::Zai, "b");
        let mut current = session(&a, "fresh");
        for key in [
            "ssh:test",
            "oauth:other:access_token",
            "oauth:login_attribution",
            "future:field",
        ] {
            current.0.insert(key.into(), "keep-exactly".into());
        }
        for key in b.credential_keys().into_iter().skip(5) {
            current.0.insert(key, "stale-b-cache".into());
        }
        let before = current.clone();
        let mut target_doc = session(&b, "new");
        for index in [2, 4, 5, 6] {
            target_doc.0.remove(&b.credential_keys()[index]);
        }
        let target = AccountSnapshot::capture(b.clone(), &target_doc).unwrap();
        let result = SwitchPlan::prepare(&current, &a, &target)
            .unwrap()
            .apply(&current)
            .unwrap();
        for key in b.credential_keys() {
            assert_eq!(result.get(&key), target_doc.get(&key));
        }
        for (key, value) in &before.0 {
            if !b.credential_keys().contains(key) {
                assert_eq!(result.get(key), Some(value.as_str()));
            }
        }
    }

    #[test]
    fn refuses_different_family_and_context() {
        let a = account(OAuthFamily::Zai, "a");
        let current = session(&a, "fresh");
        for (target_id, expected) in [
            (
                account(OAuthFamily::BigModel, "b"),
                CoreError::DifferentFamily,
            ),
            (
                AccountIdentity::new("other-context", OAuthFamily::Zai, "b").unwrap(),
                CoreError::DifferentContext,
            ),
        ] {
            let target =
                AccountSnapshot::capture(target_id.clone(), &session(&target_id, "fresh")).unwrap();
            assert!(matches!(SwitchPlan::prepare(&current,&a,&target), Err(e) if e == expected));
        }
    }

    #[test]
    fn apply_rejects_any_source_change_without_mutating_its_input() {
        let a = account(OAuthFamily::Zai, "a");
        let b = account(OAuthFamily::Zai, "b");
        let current = session(&a, "fresh");
        let target = AccountSnapshot::capture(b.clone(), &session(&b, "fresh")).unwrap();
        let plan = SwitchPlan::prepare(&current, &a, &target).unwrap();
        let mut changed = current.clone();
        changed.0.insert("unrelated:new".into(), "preserve".into());
        let original = changed.clone();
        assert!(matches!(
            plan.apply(&changed),
            Err(CoreError::SourceChanged)
        ));
        assert_same_document(&changed, &original);
    }

    #[test]
    fn capture_requires_nonempty_active_access_and_user_values() {
        let a = account(OAuthFamily::Zai, "a");
        for index in [0, 1, 3] {
            let mut incomplete = session(&a, "fresh");
            incomplete.0.remove(&a.credential_keys()[index]);
            assert!(matches!(
                AccountSnapshot::capture(a.clone(), &incomplete),
                Err(CoreError::MissingSessionCredential)
            ));
            incomplete
                .0
                .insert(a.credential_keys()[index].clone(), String::new());
            assert!(matches!(
                AccountSnapshot::capture(a.clone(), &incomplete),
                Err(CoreError::MissingSessionCredential)
            ));
        }
    }
}
