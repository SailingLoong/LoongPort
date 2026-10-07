use super::super::core::OAuthFamily;
use super::super::native::tests::{native_document, replace, TEST_CONTEXT, TEST_SECRET};
use super::*;

fn native() -> NativeCipher {
    NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap()
}

#[test]
fn checkpoint_imported_profile_remains_unverified_until_fresh_local_capture() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let imported = native
        .inspect(&native_document(OAuthFamily::Zai, "a", "imported"))
        .unwrap();
    let id = imported.identity().clone();
    let mut catalog = ProfileCatalog::default();
    catalog.upsert_unverified(imported);
    assert!(!catalog.source_verified(&id));
    let mut reopened =
        ProfileCatalog::open(&catalog.seal(&vault, &native).unwrap(), &vault, &native).unwrap();
    assert!(!reopened.source_verified(&id));
    reopened.upsert(
        native
            .inspect(&native_document(OAuthFamily::Zai, "a", "local"))
            .unwrap(),
    );
    assert!(reopened.source_verified(&id));
    let reopened =
        ProfileCatalog::open(&reopened.seal(&vault, &native).unwrap(), &vault, &native).unwrap();
    assert!(reopened.source_verified(&id));
    assert!(native
        .decrypt(
            reopened
                .get(&id)
                .unwrap()
                .scoped_document()
                .get(&id.credential_keys()[1])
                .unwrap()
        )
        .unwrap()
        .contains("_local_"));
}

#[test]
fn checkpoint_catalog_v1_is_readable_but_v2_requires_complete_provenance() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let mut catalog = ProfileCatalog::default();
    let snapshot = native
        .inspect(&native_document(OAuthFamily::Zai, "a", "local"))
        .unwrap();
    let id = snapshot.identity().clone();
    catalog.upsert(snapshot);
    let encoded = catalog.seal(&vault, &native).unwrap();
    let old = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| {
        p["version"] = 1.into();
        p.as_object_mut().unwrap().remove("unverified");
    });
    assert!(ProfileCatalog::open(&old, &vault, &native)
        .unwrap()
        .source_verified(&id));
    for values in [
        serde_json::json!(["unknown"]),
        serde_json::json!([id.opaque_id(), id.opaque_id()]),
    ] {
        let corrupt = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| p["unverified"] = values);
        assert!(ProfileCatalog::open(&corrupt, &vault, &native).is_err());
    }
    let missing = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| {
        p.as_object_mut().unwrap().remove("unverified");
    });
    assert!(ProfileCatalog::open(&missing, &vault, &native).is_err());
    let downgrade = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| p["version"] = 1.into());
    assert!(ProfileCatalog::open(&downgrade, &vault, &native).is_err());
}

#[test]
fn profile_catalog_iterates_borrowed_unique_fresh_snapshots_without_schema_changes() {
    let native = native();
    let mut catalog = ProfileCatalog::default();
    assert_eq!(catalog.profiles().count(), 0);
    for (id, version) in [("a", "old"), ("b", "fresh"), ("a", "fresh")] {
        catalog.upsert(
            native
                .inspect(&native_document(OAuthFamily::Zai, id, version))
                .unwrap(),
        );
    }
    assert_eq!(catalog.profiles().count(), 2);
    for saved in catalog.profiles() {
        assert!(std::ptr::eq(saved, catalog.get(saved.identity()).unwrap()));
        let document = saved.scoped_document();
        let key = &saved.identity().credential_keys()[1];
        assert!(native
            .decrypt(document.get(key).unwrap())
            .unwrap()
            .contains("_fresh_"));
    }
    let vault = VaultContext::generate().unwrap();
    let encoded = catalog.seal(&vault, &native).unwrap();
    let reopened = ProfileCatalog::open(&encoded, &vault, &native).unwrap();
    let ids = |catalog: &ProfileCatalog| {
        catalog
            .profiles()
            .map(|p| p.identity().opaque_id())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&catalog), ids(&reopened));
}

fn mutate_payload(
    encoded: &str,
    file: &str,
    vault: &VaultContext,
    edit: impl FnOnce(&mut serde_json::Value),
) -> String {
    let bytes = vault.open(&["file", file, "content"], encoded).unwrap();
    let mut payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    edit(&mut payload);
    vault
        .seal(
            &["file", file, "content"],
            &serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap()
}

#[test]
fn checkpoint_catalog_is_encrypted_scoped_and_updates_same_identity() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let mut catalog = ProfileCatalog::default();
    let a = native
        .inspect(&native_document(OAuthFamily::Zai, "a", "old"))
        .unwrap();
    let fresh = native
        .inspect(&native_document(OAuthFamily::Zai, "a", "fresh"))
        .unwrap();
    catalog.upsert(a);
    catalog.upsert(fresh.clone());
    assert_eq!(catalog.len(), 1);
    let encoded = catalog.seal(&vault, &native).unwrap();
    assert!(!encoded.contains("SYNTHETIC_CANARY"));
    assert!(!encoded.contains(TEST_CONTEXT));
    let plaintext = vault
        .open(&["file", PROFILE_FILE, "content"], &encoded)
        .unwrap();
    assert!(!String::from_utf8_lossy(&plaintext).contains("ssh:unrelated"));
    assert!(!String::from_utf8_lossy(&plaintext).contains("SYNTHETIC_CANARY"));
    let restored = ProfileCatalog::open(&encoded, &vault, &native).unwrap();
    assert_eq!(restored.len(), 1);
    assert!(restored.get(fresh.identity()).unwrap().scoped_document() == fresh.scoped_document());
}

#[test]
fn checkpoint_catalog_rejects_wrong_vault_native_key_context_and_file_identity() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let mut catalog = ProfileCatalog::default();
    catalog.upsert(
        native
            .inspect(&native_document(OAuthFamily::Zai, "a", "fresh"))
            .unwrap(),
    );
    let encoded = catalog.seal(&vault, &native).unwrap();
    assert!(ProfileCatalog::open(&encoded, &VaultContext::generate().unwrap(), &native).is_err());
    assert!(matches!(
        ProfileCatalog::open(
            &encoded,
            &vault,
            &NativeCipher::new("other", TEST_SECRET).unwrap()
        ),
        Err(CheckpointError::WrongContext)
    ));
    assert!(ProfileCatalog::open(
        &encoded,
        &vault,
        &NativeCipher::new(TEST_CONTEXT, "wrong-key").unwrap()
    )
    .is_err());
    assert!(SwitchCheckpoint::open(&encoded, &vault, &native).is_err());
    assert!(ProfileCatalog::open("plaintext", &vault, &native).is_err());
}

#[test]
fn checkpoint_catalog_rejects_unknown_schema_duplicate_identity_and_extra_scope() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let mut catalog = ProfileCatalog::default();
    catalog.upsert(
        native
            .inspect(&native_document(OAuthFamily::Zai, "a", "fresh"))
            .unwrap(),
    );
    let encoded = catalog.seal(&vault, &native).unwrap();
    let schema = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| p["version"] = 99.into());
    assert!(ProfileCatalog::open(&schema, &vault, &native).is_err());
    let duplicate = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| {
        let first = p["profiles"][0].clone();
        p["profiles"].as_array_mut().unwrap().push(first);
    });
    assert!(matches!(
        ProfileCatalog::open(&duplicate, &vault, &native),
        Err(CheckpointError::DuplicateIdentity)
    ));
    let scope = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| {
        p["profiles"][0]["ssh:injected"] = "opaque".into()
    });
    assert!(ProfileCatalog::open(&scope, &vault, &native).is_err());
}

#[test]
fn checkpoint_reopens_exact_preimages_for_read_only_whole_image_confirmation() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let mut current = native_document(OAuthFamily::Zai, "a", "fresh");
    let b = native
        .inspect(&native_document(OAuthFamily::Zai, "b", "fresh"))
        .unwrap();
    let b_cache = b.identity().credential_keys()[5].clone();
    current = replace(&current, &b_cache, "old-native-b-cache-opaque".into());
    let checkpoint = SwitchCheckpoint::prepare(&current, &b, &native).unwrap();
    assert!(
        checkpoint.fresh_source().identity()
            == &AccountIdentity::new(TEST_CONTEXT, OAuthFamily::Zai, "a").unwrap()
    );
    let published = checkpoint.apply(&current).unwrap();
    let encoded = checkpoint.seal(&vault, &native).unwrap();
    let payload = vault
        .open(&["file", JOURNAL_FILE, "content"], &encoded)
        .unwrap();
    assert!(!String::from_utf8_lossy(&payload).contains("ssh:unrelated"));
    let reopened = SwitchCheckpoint::open(&encoded, &vault, &native).unwrap();
    assert!(matches!(
        reopened.apply(&current),
        Err(CheckpointError::RecoveryOnly)
    ));
    let changed = replace(
        &published,
        "ssh:unrelated",
        "newer-third-party-value".into(),
    );
    let unchanged = changed.to_bytes().unwrap();
    assert_eq!(
        reopened.match_touched_image(&changed),
        Some(TouchedImage::After)
    );
    assert_eq!(changed.to_bytes().unwrap(), unchanged);
    assert_eq!(
        reopened.match_touched_image(&current),
        Some(TouchedImage::Before)
    );
    assert_eq!(
        reopened.plan.target_preimages().get(&b_cache),
        Some(&Some("old-native-b-cache-opaque".into()))
    );
    let stale_cache = replace(&current, &b_cache, "different-b-cache".into());
    assert_eq!(reopened.match_touched_image(&stale_cache), None);
}

#[test]
fn checkpoint_rejects_invalid_preimage_scope_shared_values_and_unknown_phase() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let current = native_document(OAuthFamily::Zai, "a", "fresh");
    let b = native
        .inspect(&native_document(OAuthFamily::Zai, "b", "fresh"))
        .unwrap();
    let encoded = SwitchCheckpoint::prepare(&current, &b, &native)
        .unwrap()
        .seal(&vault, &native)
        .unwrap();
    for (field, value) in [
        ("phase", serde_json::json!("future")),
        ("origin", serde_json::json!("live")),
    ] {
        let changed = mutate_payload(&encoded, JOURNAL_FILE, &vault, |p| p[field] = value);
        assert!(SwitchCheckpoint::open(&changed, &vault, &native).is_err());
    }
    let scope = mutate_payload(&encoded, JOURNAL_FILE, &vault, |p| {
        p["before"]["ssh:injected"] = serde_json::Value::Null
    });
    assert!(SwitchCheckpoint::open(&scope, &vault, &native).is_err());
    let inconsistent = mutate_payload(&encoded, JOURNAL_FILE, &vault, |p| {
        p["before"]["oauth:active_provider"] = "different".into()
    });
    assert!(SwitchCheckpoint::open(&inconsistent, &vault, &native).is_err());
}

#[test]
fn checkpoint_every_reopened_phase_is_inspection_only_and_keeps_exact_images() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let current = native_document(OAuthFamily::Zai, "a", "fresh");
    let b = native
        .inspect(&native_document(OAuthFamily::Zai, "b", "fresh"))
        .unwrap();
    for phase in [
        TransactionPhase::Prepared,
        TransactionPhase::Captured,
        TransactionPhase::CredentialsPublished,
        TransactionPhase::CommitUncertain,
        TransactionPhase::Committed,
    ] {
        let mut checkpoint = SwitchCheckpoint::prepare(&current, &b, &native).unwrap();
        let after = checkpoint.apply(&current).unwrap();
        checkpoint.set_phase(phase).unwrap();
        let reopened =
            SwitchCheckpoint::open(&checkpoint.seal(&vault, &native).unwrap(), &vault, &native)
                .unwrap();
        assert_eq!(reopened.phase, phase);
        assert!(matches!(
            reopened.apply(&current),
            Err(CheckpointError::RecoveryOnly)
        ));
        assert!(matches!(
            reopened.apply(&after),
            Err(CheckpointError::RecoveryOnly)
        ));
        assert_eq!(
            reopened.match_touched_image(&current),
            Some(TouchedImage::Before)
        );
        assert_eq!(
            reopened.match_touched_image(&after),
            Some(TouchedImage::After)
        );
        let refreshed = native_document(OAuthFamily::Zai, "b", "newer");
        assert_eq!(reopened.match_touched_image(&refreshed), None);
    }
    let a = native.inspect(&current).unwrap();
    assert!(matches!(
        SwitchCheckpoint::prepare(&current, &a, &native),
        Err(CheckpointError::NoChange)
    ));
}

#[test]
fn checkpoint_phase_is_monotonic_and_published_states_cannot_apply() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let current = native_document(OAuthFamily::Zai, "a", "fresh");
    let b = native
        .inspect(&native_document(OAuthFamily::Zai, "b", "fresh"))
        .unwrap();
    let phases = [
        TransactionPhase::Prepared,
        TransactionPhase::Captured,
        TransactionPhase::CredentialsPublished,
        TransactionPhase::CommitUncertain,
        TransactionPhase::Committed,
    ];
    for phase in phases {
        for next in phases {
            let mut checkpoint = SwitchCheckpoint::prepare(&current, &b, &native).unwrap();
            checkpoint.set_phase(phase).unwrap();
            let changed = checkpoint.set_phase(next);
            let expected = if next < phase {
                assert_eq!(changed, Err(CheckpointError::InvalidPhase));
                phase
            } else {
                changed.unwrap();
                next
            };
            assert_eq!(checkpoint.phase, expected);
            if expected >= TransactionPhase::CredentialsPublished {
                assert!(matches!(
                    checkpoint.apply(&current),
                    Err(CheckpointError::RecoveryOnly)
                ));
            } else {
                assert!(checkpoint.apply(&current).is_ok());
            }
            let reopened =
                SwitchCheckpoint::open(&checkpoint.seal(&vault, &native).unwrap(), &vault, &native)
                    .unwrap();
            assert_eq!(reopened.phase, expected);
        }
    }
}

#[test]
fn checkpoint_rejects_oversized_envelopes_before_decoding() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let too_large = "x".repeat(16 * 1024 * 1024 + 1);
    assert!(matches!(
        ProfileCatalog::open(&too_large, &vault, &native),
        Err(CheckpointError::ResourceLimit)
    ));
    assert!(matches!(
        SwitchCheckpoint::open(&too_large, &vault, &native),
        Err(CheckpointError::ResourceLimit)
    ));
}

#[test]
fn checkpoint_transaction_binding_roundtrips_and_cannot_be_rebound() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let current = native_document(OAuthFamily::Zai, "a", "fresh");
    let target = native
        .inspect(&native_document(OAuthFamily::Zai, "b", "saved"))
        .unwrap();
    let mut checkpoint = SwitchCheckpoint::prepare(&current, &target, &native).unwrap();
    let binding = TransactionBinding {
        operation: uuid::Uuid::new_v4().to_string(),
        native_root: [11, 22],
        vault_root: [33, 44],
        source_revision: [55; 32],
        source_profile_revision: [66; 32],
    };
    checkpoint.bind(binding.clone()).unwrap();
    assert!(checkpoint.bind(binding.clone()).is_err());
    let encoded = checkpoint.seal(&vault, &native).unwrap();
    let mut reopened = SwitchCheckpoint::open(&encoded, &vault, &native).unwrap();
    assert!(reopened.binding() == Some(&binding));
    assert!(reopened.bind(binding).is_err());
    for field in ["operation", "native_root", "source_revision"] {
        let invalid = mutate_payload(&encoded, JOURNAL_FILE, &vault, |payload| {
            payload["binding"][field] = serde_json::json!("invalid");
        });
        assert!(SwitchCheckpoint::open(&invalid, &vault, &native).is_err());
    }
}

#[test]
fn checkpoint_matches_only_one_whole_touched_image_for_both_families() {
    let native = native();
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let current = native_document(family, "a", "fresh");
        let target = native
            .inspect(&native_document(family, "b", "saved"))
            .unwrap();
        let checkpoint = SwitchCheckpoint::prepare(&current, &target, &native).unwrap();
        let after = checkpoint.apply(&current).unwrap();
        assert_eq!(
            checkpoint.match_touched_image(&current),
            Some(TouchedImage::Before)
        );
        assert_eq!(
            checkpoint.match_touched_image(&after),
            Some(TouchedImage::After)
        );
        let mixed = replace(
            &after,
            &target.identity().credential_keys()[1],
            current
                .get(&target.identity().credential_keys()[1])
                .unwrap()
                .to_owned(),
        );
        assert!(native.inspect(&mixed).unwrap().identity() == target.identity());
        assert_eq!(checkpoint.match_touched_image(&mixed), None);
        let refresh = native_document(family, "b", "new-refresh");
        assert_eq!(checkpoint.match_touched_image(&refresh), None);
        let third = native_document(family, "c", "official-login");
        assert_eq!(checkpoint.match_touched_image(&third), None);
        let unrelated = replace(&after, "ssh:unrelated", "new-value".into());
        assert_eq!(
            checkpoint.match_touched_image(&unrelated),
            Some(TouchedImage::After)
        );
        let source_only = checkpoint.fresh_source().identity().credential_keys()[5].clone();
        let untouched = replace(&after, &source_only, "new-untouched-value".into());
        assert_eq!(
            checkpoint.match_touched_image(&untouched),
            Some(TouchedImage::After)
        );
    }
}

#[test]
fn checkpoint_whole_image_includes_optional_absence_and_target_specific_preimages() {
    let native = native();
    let mut current = native_document(OAuthFamily::Zai, "a", "fresh");
    let target_document = native_document(OAuthFamily::Zai, "b", "saved");
    let id = AccountIdentity::new(TEST_CONTEXT, OAuthFamily::Zai, "b").unwrap();
    let keys = id.credential_keys();
    let mut entries = target_document.entries().clone();
    entries.remove(&keys[2]);
    entries.remove(&keys[6]);
    let target_document =
        CredentialDocument::parse(&serde_json::to_vec(&entries).unwrap()).unwrap();
    let target = native.inspect(&target_document).unwrap();
    current = replace(&current, &keys[5], "opaque-existing-b-preimage".into());
    let checkpoint = SwitchCheckpoint::prepare(&current, &target, &native).unwrap();
    let after = checkpoint.apply(&current).unwrap();
    assert_eq!(
        checkpoint.match_touched_image(&current),
        Some(TouchedImage::Before)
    );
    assert_eq!(
        checkpoint.match_touched_image(&after),
        Some(TouchedImage::After)
    );
    let altered_before = replace(&current, &keys[5], "different-existing-b".into());
    assert_eq!(checkpoint.match_touched_image(&altered_before), None);
    let missing_is_not_empty = replace(&after, &keys[6], String::new());
    assert_eq!(checkpoint.match_touched_image(&missing_is_not_empty), None);
    let mixed_optional = replace(&after, &keys[2], current.get(&keys[2]).unwrap().into());
    assert_eq!(checkpoint.match_touched_image(&mixed_optional), None);
}

#[test]
fn checkpoint_rejects_every_partial_image_and_changed_touched_value_after_reopen() {
    let vault = VaultContext::generate().unwrap();
    let native = native();
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        for cache_present in [false, true] {
            let mut before = native_document(family, "a", "fresh");
            let identity = AccountIdentity::new(TEST_CONTEXT, family, "b").unwrap();
            let keys = identity.credential_keys();
            if cache_present {
                before = replace(&before, &keys[5], "old-opaque-b-cache".into());
            }
            let mut entries = native_document(family, "b", "saved").entries().clone();
            entries.remove(&keys[2]);
            entries.remove(&keys[6]);
            let target_document =
                CredentialDocument::parse(&serde_json::to_vec(&entries).unwrap()).unwrap();
            let target = native.inspect(&target_document).unwrap();
            let checkpoint = SwitchCheckpoint::prepare(&before, &target, &native).unwrap();
            let after = checkpoint.apply(&before).unwrap();
            let checkpoint =
                SwitchCheckpoint::open(&checkpoint.seal(&vault, &native).unwrap(), &vault, &native)
                    .unwrap();
            let changed_keys: Vec<_> = keys
                .iter()
                .filter(|key| before.get(key) != after.get(key))
                .collect();
            let full_mask = (1 << changed_keys.len()) - 1;
            for mask in 0..=full_mask {
                let mut entries = before.entries().clone();
                for (index, key) in changed_keys.iter().enumerate() {
                    if mask & (1 << index) != 0 {
                        match after.get(key) {
                            Some(value) => {
                                entries.insert((*key).clone(), value.into());
                            }
                            None => {
                                entries.remove(*key);
                            }
                        }
                    }
                }
                entries.insert("ssh:unrelated".into(), "new-unrelated-value".into());
                let partial =
                    CredentialDocument::parse(&serde_json::to_vec(&entries).unwrap()).unwrap();
                let unchanged = partial.to_bytes().unwrap();
                let expected = if mask == 0 {
                    Some(TouchedImage::Before)
                } else if mask == full_mask {
                    Some(TouchedImage::After)
                } else {
                    None
                };
                assert_eq!(checkpoint.match_touched_image(&partial), expected);
                assert_eq!(partial.to_bytes().unwrap(), unchanged);
            }
            for image in [&before, &after] {
                for key in &keys {
                    let changed = replace(image, key, "unknown-touched-value".into());
                    let unchanged = changed.to_bytes().unwrap();
                    assert_eq!(checkpoint.match_touched_image(&changed), None);
                    assert_eq!(changed.to_bytes().unwrap(), unchanged);
                }
            }
        }
    }
}

#[test]
fn checkpoint_journal_authentication_rejects_wrong_keys_context_and_legacy_recovery_layout() {
    use super::super::recovery::RecoveryLedger;
    use crate::secrets::owned_file::RECOVERY_FILE;
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let current = native_document(OAuthFamily::Zai, "a", "fresh");
    let target = native
        .inspect(&native_document(OAuthFamily::Zai, "b", "saved"))
        .unwrap();
    let encoded = SwitchCheckpoint::prepare(&current, &target, &native)
        .unwrap()
        .seal(&vault, &native)
        .unwrap();
    assert!(matches!(
        SwitchCheckpoint::open(&encoded, &VaultContext::generate().unwrap(), &native),
        Err(CheckpointError::Vault)
    ));
    assert!(matches!(
        SwitchCheckpoint::open(
            &encoded,
            &vault,
            &NativeCipher::new(TEST_CONTEXT, "wrong-key").unwrap()
        ),
        Err(CheckpointError::Native(_))
    ));
    assert!(matches!(
        SwitchCheckpoint::open(
            &encoded,
            &vault,
            &NativeCipher::new("wrong-context", TEST_SECRET).unwrap()
        ),
        Err(CheckpointError::WrongContext)
    ));
    let payload = vault
        .open(&["file", JOURNAL_FILE, "content"], &encoded)
        .unwrap();
    let old_recovery = vault
        .seal(&["file", RECOVERY_FILE, "content"], &payload)
        .unwrap();
    assert!(matches!(
        SwitchCheckpoint::open(&old_recovery, &vault, &native),
        Err(CheckpointError::Vault)
    ));
    assert!(RecoveryLedger::open(&old_recovery, &vault).is_err());
    assert!(RecoveryLedger::open(&encoded, &vault).is_err());
}

#[test]
fn login_receipts_are_not_evicted_by_later_successful_saves() {
    let mut catalog = ProfileCatalog::default();
    let first = uuid::Uuid::new_v4().to_string();
    catalog
        .record_login(LoginReceipt {
            request_id: first.clone(),
            account_id: "a".repeat(64),
            candidate_revision: "b".repeat(64),
            outcome: super::super::transaction::CaptureCommitOutcome::Saved,
        })
        .unwrap();
    for _ in 0..16 {
        catalog
            .record_login(LoginReceipt {
                request_id: uuid::Uuid::new_v4().to_string(),
                account_id: "a".repeat(64),
                candidate_revision: "b".repeat(64),
                outcome: super::super::transaction::CaptureCommitOutcome::Refreshed,
            })
            .unwrap();
    }
    assert!(catalog.login_receipt(&first).is_some());
}

#[test]
fn login_receipt_capacity_rejects_new_work_without_forgetting_old_results() {
    let mut catalog = ProfileCatalog::default();
    let first = uuid::Uuid::new_v4().to_string();
    for index in 0..MAX_LOGIN_RECEIPTS {
        catalog
            .record_login(LoginReceipt {
                request_id: if index == 0 {
                    first.clone()
                } else {
                    uuid::Uuid::new_v4().to_string()
                },
                account_id: "a".repeat(64),
                candidate_revision: "b".repeat(64),
                outcome: super::super::transaction::CaptureCommitOutcome::Saved,
            })
            .unwrap();
    }
    assert_eq!(
        catalog.record_login(LoginReceipt {
            request_id: uuid::Uuid::new_v4().to_string(),
            account_id: "a".repeat(64),
            candidate_revision: "b".repeat(64),
            outcome: super::super::transaction::CaptureCommitOutcome::Saved
        }),
        Err(CheckpointError::ResourceLimit)
    );
    let vault = VaultContext::generate().unwrap();
    let native = native();
    let opened =
        ProfileCatalog::open(&catalog.seal(&vault, &native).unwrap(), &vault, &native).unwrap();
    assert!(opened.login_receipt(&first).is_some());
    assert_eq!(opened.login_receipts.len(), MAX_LOGIN_RECEIPTS);
}
