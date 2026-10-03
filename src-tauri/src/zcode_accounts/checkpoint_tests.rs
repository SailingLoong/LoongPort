use super::super::core::OAuthFamily;
use super::super::native::tests::{native_document, replace, TEST_CONTEXT, TEST_SECRET};
use super::*;

fn native() -> NativeCipher {
    NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap()
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
    let schema = mutate_payload(&encoded, PROFILE_FILE, &vault, |p| p["version"] = 2.into());
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
fn checkpoint_reopens_and_restores_exact_preimage_without_whole_file_replay() {
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
    match reopened.recover(&changed, JournalOrigin::Live).unwrap() {
        RecoveryOutcome::Restore(restored) => {
            assert_eq!(restored.get(&b_cache), Some("old-native-b-cache-opaque"));
            assert_eq!(
                restored.get("ssh:unrelated"),
                Some("newer-third-party-value")
            );
            for key in b.identity().credential_keys() {
                assert_eq!(restored.get(&key), current.get(&key));
            }
        }
        _ => panic!("expected scoped restoration"),
    }
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
fn checkpoint_origin_and_commit_phase_control_recovery_without_native_write() {
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
        checkpoint.set_phase(phase).unwrap();
        let reopened =
            SwitchCheckpoint::open(&checkpoint.seal(&vault, &native).unwrap(), &vault, &native)
                .unwrap();
        assert!(matches!(
            reopened.recover(&current, JournalOrigin::Restored),
            Ok(RecoveryOutcome::Quarantine)
        ));
        match phase {
            TransactionPhase::Committed => assert!(matches!(
                reopened.recover(&current, JournalOrigin::Live),
                Ok(RecoveryOutcome::CleanupOnly)
            )),
            TransactionPhase::CommitUncertain => assert!(matches!(
                reopened.recover(&current, JournalOrigin::Live),
                Ok(RecoveryOutcome::ReconcileCommit)
            )),
            _ => assert!(matches!(
                reopened.recover(&current, JournalOrigin::Live),
                Ok(RecoveryOutcome::Restore(_))
            )),
        }
    }
    let a = native.inspect(&current).unwrap();
    assert!(matches!(
        SwitchCheckpoint::prepare(&current, &a, &native),
        Err(CheckpointError::NoChange)
    ));
}

#[test]
fn checkpoint_cannot_downgrade_a_known_commit_marker() {
    let native = native();
    let current = native_document(OAuthFamily::Zai, "a", "fresh");
    let b = native
        .inspect(&native_document(OAuthFamily::Zai, "b", "fresh"))
        .unwrap();
    let mut checkpoint = SwitchCheckpoint::prepare(&current, &b, &native).unwrap();
    checkpoint.set_phase(TransactionPhase::Committed).unwrap();
    assert!(matches!(
        checkpoint.set_phase(TransactionPhase::Captured),
        Err(CheckpointError::InvalidPhase)
    ));
    assert!(matches!(
        checkpoint.recover(&current, JournalOrigin::Live),
        Ok(RecoveryOutcome::CleanupOnly)
    ));
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
