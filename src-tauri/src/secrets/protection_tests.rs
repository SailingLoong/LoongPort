use super::*;
fn plaintext() -> ProtectionRecord {
    ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: false,
        storage: ProtectionStorage::Plaintext {
            identity: PlaintextIdentity {
                format_version: 1,
                store_id: "11111111-1111-4111-8111-111111111111".into(),
                generation_id: "22222222-2222-4222-8222-222222222222".into(),
                revision: 1,
            },
        },
    }
}
#[test]
fn explicit_plaintext_record_is_strict_and_prompt_completion_is_durable() {
    let mut value = plaintext();
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(ProtectionRecord::parse(&bytes).unwrap(), value);
    assert!(!value.should_prompt(false));
    assert!(value.should_prompt(true));
    value.choice_complete = true;
    let reopened = ProtectionRecord::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(!reopened.should_prompt(true));
    assert!(!reopened.should_prompt(false));
    for change in [
        serde_json::json!({"formatVersion":2,"choiceComplete":false,"storage":{"mode":"plaintext"}}),
        serde_json::json!({"formatVersion":1,"choiceComplete":true,"storage":{"mode":"unknown"}}),
        serde_json::json!({"formatVersion":1,"choiceComplete":false,"storage":{"mode":"plaintext"},"key":"not allowed"}),
    ] {
        assert!(ProtectionRecord::parse(&serde_json::to_vec(&change).unwrap()).is_err());
    }
}
#[test]
fn protection_mode_never_downgrades_missing_wrong_or_present_vault() {
    let vault =
        super::super::VaultContext::create_with_user_key(zeroize::Zeroizing::new(vec![4; 32]))
            .unwrap();
    let metadata = vault.metadata();
    let encrypted = ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: true,
        storage: ProtectionStorage::Encrypted {
            vault_id: metadata.vault_id.clone(),
            key_id: metadata.key_id.clone(),
        },
    };
    encrypted.validate_vault(Some(metadata)).unwrap();
    encrypted.matches_context(&vault.clone().into()).unwrap();
    assert!(encrypted.validate_vault(None).is_err());
    let other =
        super::super::VaultContext::create_with_user_key(zeroize::Zeroizing::new(vec![5; 32]))
            .unwrap();
    assert!(encrypted.validate_vault(Some(other.metadata())).is_err());
    let plain = plaintext();
    plain.validate_vault(None).unwrap();
    assert!(plain.validate_vault(Some(metadata)).is_err());
    assert!(plain.matches_context(&vault.into()).is_err());
}
#[test]
fn disk_record_missing_is_not_a_plaintext_decision_and_corruption_is_not_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read(dir.path()).unwrap().is_none());
    std::fs::write(dir.path().join(FILE), b"broken").unwrap();
    assert!(read(dir.path()).is_err());
    assert_eq!(std::fs::read(dir.path().join(FILE)).unwrap(), b"broken");
    std::fs::write(
        dir.path().join(FILE),
        serde_json::to_vec(&plaintext()).unwrap(),
    )
    .unwrap();
    assert_eq!(read(dir.path()).unwrap(), Some(plaintext()));
}

#[test]
fn cache_none_some_error_and_wrong_identity_remain_distinct() {
    let key = zeroize::Zeroizing::new(vec![7; 32]);
    let vault = super::super::VaultContext::create_with_user_key(key.clone()).unwrap();
    let m = vault.metadata();
    let record = ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: true,
        storage: ProtectionStorage::Encrypted {
            vault_id: m.vault_id.clone(),
            key_id: m.key_id.clone(),
        },
    };
    let opened = open_context(&record, Some(m), None, |v, k| {
        assert_eq!(v, m.vault_id);
        assert_eq!(k, m.key_id);
        Ok(Some(key.clone()))
    })
    .unwrap();
    assert_eq!(
        opened.identity(),
        StorageContext::from(vault.clone()).identity()
    );
    assert!(
        matches!(open_context(&record,Some(m),None,|_,_|Ok(None)),Err(AppError::Config(code)) if code=="secret.key_missing")
    );
    assert!(
        matches!(open_context(&record,Some(m),None,|_,_|Err(AppError::Config("secret.store_unavailable".into()))),Err(AppError::Config(code)) if code=="secret.store_unavailable")
    );
    assert!(open_context(&record, Some(m), None, |_, _| Ok(Some(
        zeroize::Zeroizing::new(vec![8; 32])
    )))
    .is_err());
    let wrong = super::super::VaultContext::create_with_user_key(key.clone()).unwrap();
    assert!(
        open_context(&record, Some(wrong.metadata()), None, |_, _| panic!(
            "identity must precede cache IO"
        ))
        .is_err()
    );
}
#[test]
fn manual_same_key_and_explicit_plaintext_need_no_cache_access() {
    let key = zeroize::Zeroizing::new(vec![9; 32]);
    let vault = super::super::VaultContext::create_with_user_key(key.clone()).unwrap();
    let m = vault.metadata();
    let record = ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: true,
        storage: ProtectionStorage::Encrypted {
            vault_id: m.vault_id.clone(),
            key_id: m.key_id.clone(),
        },
    };
    open_context(&record, Some(m), Some(key), |_, _| {
        panic!("manual recovery does not depend on cache")
    })
    .unwrap();
    let record = plaintext();
    let opened = open_context(&record, None, None, |_, _| {
        panic!("plaintext never creates or reads a key")
    })
    .unwrap();
    assert!(opened.is_plaintext());
    assert!(open_context(
        &record,
        None,
        Some(zeroize::Zeroizing::new(vec![1; 32])),
        |_, _| panic!()
    )
    .is_err());
}

#[test]
fn proven_fresh_scope_publishes_real_plaintext_without_a_vault() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let device = tmp.path().join("device");
    let proof = FreshScope::capture(&root, &device, &[]).unwrap();
    assert!(!root.exists());
    assert!(!device.exists());
    let record = proof.publish_initial_plaintext().unwrap();
    assert!(!record.choice_complete);
    assert!(matches!(
        record.storage,
        ProtectionStorage::Plaintext { .. }
    ));
    assert_eq!(read(&root).unwrap(), Some(record));
    assert!(!root.join("vault.json").exists());
    assert!(FreshScope::capture(&root, &device, &[]).is_err());
}
#[test]
fn existing_or_changed_data_cannot_be_reclassified_as_fresh_plaintext() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("data");
    let device = tmp.path().join("device");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&device).unwrap();
    let proof = FreshScope::capture(&root, &device, &[]).unwrap();
    std::fs::write(device.join("settings.json"), b"old unreadable data").unwrap();
    assert!(proof.verify_unchanged().is_err());
    assert!(proof.publish_initial_plaintext().is_err());
    assert!(!root.join(FILE).exists());
    assert_eq!(
        std::fs::read(device.join("settings.json")).unwrap(),
        b"old unreadable data"
    );
    for name in [
        "vault.json",
        "cc-switch.db",
        "unknown-file",
        ".vault-transition",
    ] {
        let owned = tmp.path().join(name.replace('.', "_"));
        std::fs::create_dir(&owned).unwrap();
        std::fs::write(owned.join(name), b"original").unwrap();
        assert!(FreshScope::capture(&owned, &owned, &[]).is_err());
        assert_eq!(std::fs::read(owned.join(name)).unwrap(), b"original");
    }
}
#[cfg(unix)]
#[test]
fn fresh_scope_rejects_ancestor_and_diagnostic_aliases_without_touching_targets() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
    let alias = tmp.path().join("alias");
    symlink(&real, &alias).unwrap();
    assert!(FreshScope::capture(&alias.join("missing"), &real, &[]).is_err());
    assert!(!real.join("missing").exists());
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let root = tmp.path().join("data");
    std::fs::create_dir(&root).unwrap();
    symlink(&real, root.join("logs")).unwrap();
    assert!(FreshScope::capture(&root, &root, &["logs".into()]).is_err());
    assert!(std::fs::read_dir(&real).unwrap().next().is_none());
}

#[test]
fn user_text_cache_authentication_is_selected_only_by_persisted_metadata() {
    for text in [
        " 中文🔑 ",
        "AB".repeat(32).as_str(),
        "z".repeat(32).as_str(),
        "1234567",
    ] {
        let vault = super::super::VaultContext::create_generation_with_user_text(
            uuid::Uuid::new_v4().to_string(),
            1,
            text,
        )
        .unwrap();
        let metadata = vault.metadata();
        let record = ProtectionRecord::encrypted(&vault, None).unwrap();
        let cached = super::super::key_store::user_text::encode(text).unwrap();
        let opened = open_context(&record, Some(metadata), None, |_, _| Ok(Some(cached))).unwrap();
        assert_eq!(
            opened.encrypted().unwrap().export_key().as_slice(),
            vault.export_key().as_slice()
        );
        assert!(
            open_context(&record, Some(metadata), None, |_, _| Ok(Some(
                vault.export_key()
            )))
            .is_err(),
            "new metadata must not silently accept derived32 as a text cache"
        );
    }
    // 32 raw legacy bytes can resemble a new cache prefix; metadata, not shape, wins.
    let raw = super::super::key_store::user_text::encode("1234567").unwrap();
    assert_eq!(raw.len(), 32);
    let legacy = super::super::VaultContext::create_with_user_key(raw.clone()).unwrap();
    let m = legacy.metadata();
    let record = ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: true,
        storage: ProtectionStorage::Encrypted {
            vault_id: m.vault_id.clone(),
            key_id: m.key_id.clone(),
        },
    };
    let opened = open_context(&record, Some(m), None, |_, _| Ok(Some(raw))).unwrap();
    assert_eq!(
        opened.encrypted().unwrap().export_key().as_slice(),
        legacy.export_key().as_slice()
    );
}

#[test]
fn pending_plaintext_cleanup_cannot_be_stripped_to_publish_an_available_scope() {
    let vault = super::super::VaultContext::create_generation_with_user_text(
        uuid::Uuid::new_v4().to_string(),
        1,
        "x",
    )
    .unwrap();
    let record =
        ProtectionRecord::encrypted(&vault, Some(r#"{"source":"synthetic-previous-scope"}"#))
            .unwrap();
    assert!(
        matches!(open_context(&record,Some(vault.metadata()),Some(vault.export_key()),|_,_|panic!()),Err(AppError::Config(code)) if code=="protection.cleanup_required")
    );
    let mut stripped = record.clone();
    stripped.cleanup = None;
    assert!(open_context(
        &stripped,
        Some(vault.metadata()),
        Some(vault.export_key()),
        |_, _| panic!()
    )
    .is_err());
    stripped.commitment = None;
    assert!(open_context(
        &stripped,
        Some(vault.metadata()),
        Some(vault.export_key()),
        |_, _| panic!()
    )
    .is_err());
    assert!(record.cleanup.as_ref().unwrap().starts_with("lpenc"));
    assert!(!serde_json::to_string(&record)
        .unwrap()
        .contains("synthetic-previous-scope"));
}
#[test]
fn completed_text_choice_authenticates_its_flags_and_exact_target_generation() {
    let vault = super::super::VaultContext::create_generation_with_user_text(
        uuid::Uuid::new_v4().to_string(),
        1,
        " ",
    )
    .unwrap();
    let record = ProtectionRecord::encrypted(&vault, None).unwrap();
    open_context(
        &record,
        Some(vault.metadata()),
        Some(vault.export_key()),
        |_, _| panic!(),
    )
    .unwrap();
    let mut wrong = record.clone();
    wrong.cleanup = Some(
        vault
            .seal(&["protection-record", "cleanup"], b"other")
            .unwrap(),
    );
    assert!(open_context(
        &wrong,
        Some(vault.metadata()),
        Some(vault.export_key()),
        |_, _| panic!()
    )
    .is_err());
    wrong = record.clone();
    wrong.commitment = Some("lpenc:tampered".into());
    assert!(open_context(
        &wrong,
        Some(vault.metadata()),
        Some(vault.export_key()),
        |_, _| panic!()
    )
    .is_err());
    let other = super::super::VaultContext::create_generation_with_user_text(
        vault.metadata().vault_id.clone(),
        2,
        " ",
    )
    .unwrap();
    assert!(open_context(
        &record,
        Some(other.metadata()),
        Some(other.export_key()),
        |_, _| panic!()
    )
    .is_err());
}
