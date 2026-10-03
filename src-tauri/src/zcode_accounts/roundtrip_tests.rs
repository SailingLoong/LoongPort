//! Explicit temporary-directory composition tests, not native durability tests.
use super::checkpoint::{
    ProfileCatalog, RecoveryOutcome, SwitchCheckpoint, JOURNAL_FILE, PROFILE_FILE,
};
use super::core::{CredentialDocument, JournalOrigin, OAuthFamily, TransactionPhase};
use super::native::tests::{native_document, replace, TEST_SECRET};
use super::native::NativeCipher;
use crate::secrets::VaultContext;
use std::path::Path;

fn read_document(path: &Path) -> CredentialDocument {
    CredentialDocument::parse(&std::fs::read(path).unwrap()).unwrap()
}

fn reopen_vault(vault: VaultContext) -> VaultContext {
    // Synthetic test custody only: the key remains in memory, never in a keychain/file.
    let metadata = vault.metadata().clone();
    let key = vault.export_key();
    drop(vault);
    VaultContext::from_key(metadata, key).unwrap()
}

fn merge_synthetic_refresh(
    current: &CredentialDocument,
    fresh: &CredentialDocument,
    native: &NativeCipher,
) -> CredentialDocument {
    let mut result = current.clone();
    for key in native.inspect(fresh).unwrap().identity().credential_keys() {
        result = replace(&result, &key, fresh.get(&key).unwrap().into());
    }
    result
}

#[test]
fn encrypted_roundtrip_a_b_a_reopens_and_retains_both_latest_refreshes() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let native = NativeCipher::new(root.to_str().unwrap(), TEST_SECRET).unwrap();
        let mut vault = VaultContext::generate().unwrap();
        let path = root.join("synthetic-credentials.json");
        let a0 = native_document(family, "a", "initial");
        let a_prime = native_document(family, "a", "refreshed-a");
        let b0 = native_document(family, "b", "initial");
        let b_prime = native_document(family, "b", "refreshed-b");
        let a_identity = native.inspect(&a0).unwrap().identity().clone();
        let b_identity = native.inspect(&b0).unwrap().identity().clone();
        let mut catalog = ProfileCatalog::default();
        catalog.upsert(native.inspect(&a0).unwrap());
        catalog.upsert(native.inspect(&b0).unwrap());
        std::fs::write(&path, a_prime.to_bytes().unwrap()).unwrap();
        std::fs::write(
            root.join("telemetry-state.json"),
            b"SYNTHETIC-DO-NOT-CHANGE",
        )
        .unwrap();

        let current = read_document(&path);
        let mut to_b =
            SwitchCheckpoint::prepare(&current, catalog.get(&b_identity).unwrap(), &native)
                .unwrap();
        catalog.upsert(to_b.fresh_source().clone());
        std::fs::write(
            root.join(PROFILE_FILE),
            catalog.seal(&vault, &native).unwrap(),
        )
        .unwrap();
        to_b.set_phase(TransactionPhase::Captured).unwrap();
        std::fs::write(root.join(JOURNAL_FILE), to_b.seal(&vault, &native).unwrap()).unwrap();
        std::fs::write(&path, to_b.apply(&current).unwrap().to_bytes().unwrap()).unwrap();
        to_b.set_phase(TransactionPhase::Committed).unwrap();
        std::fs::write(root.join(JOURNAL_FILE), to_b.seal(&vault, &native).unwrap()).unwrap();
        assert!(native.inspect(&read_document(&path)).unwrap().identity() == &b_identity);
        let refreshed_b = merge_synthetic_refresh(&read_document(&path), &b_prime, &native);
        std::fs::write(&path, refreshed_b.to_bytes().unwrap()).unwrap();
        drop(to_b);
        drop(catalog);
        vault = reopen_vault(vault);

        let committed = SwitchCheckpoint::open(
            &std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap(),
            &vault,
            &native,
        )
        .unwrap();
        assert!(matches!(
            committed.recover(&read_document(&path), JournalOrigin::Live),
            Ok(RecoveryOutcome::CleanupOnly)
        ));
        std::fs::remove_file(root.join(JOURNAL_FILE)).unwrap();
        let mut catalog = ProfileCatalog::open(
            &std::fs::read_to_string(root.join(PROFILE_FILE)).unwrap(),
            &vault,
            &native,
        )
        .unwrap();
        let to_a = SwitchCheckpoint::prepare(
            &read_document(&path),
            catalog.get(&a_identity).unwrap(),
            &native,
        )
        .unwrap();
        catalog.upsert(to_a.fresh_source().clone());
        std::fs::write(
            root.join(PROFILE_FILE),
            catalog.seal(&vault, &native).unwrap(),
        )
        .unwrap();
        let result = to_a.apply(&read_document(&path)).unwrap();
        std::fs::write(&path, result.to_bytes().unwrap()).unwrap();
        assert!(native.inspect(&read_document(&path)).unwrap().identity() == &a_identity);
        for key in a_identity.credential_keys() {
            assert_eq!(result.get(&key), a_prime.get(&key));
        }
        let stored = ProfileCatalog::open(
            &std::fs::read_to_string(root.join(PROFILE_FILE)).unwrap(),
            &vault,
            &native,
        )
        .unwrap();
        assert_eq!(stored.len(), 2);
        for key in b_identity.credential_keys() {
            assert_eq!(
                stored.get(&b_identity).unwrap().scoped_document().get(&key),
                b_prime.get(&key)
            );
        }
        let profile_bytes = std::fs::read_to_string(root.join(PROFILE_FILE)).unwrap();
        assert!(!profile_bytes.contains("SYNTHETIC_CANARY"));
        assert!(!profile_bytes.contains(root.to_str().unwrap()));
        assert_eq!(
            std::fs::read(root.join("telemetry-state.json")).unwrap(),
            b"SYNTHETIC-DO-NOT-CHANGE"
        );
    }
}

#[test]
fn encrypted_roundtrip_precommit_recovery_refreshes_old_catalog_and_preserves_unrelated_writes() {
    for phase in [
        TransactionPhase::Prepared,
        TransactionPhase::Captured,
        TransactionPhase::CredentialsPublished,
    ] {
        for cache_present in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let native = NativeCipher::new(root.to_str().unwrap(), TEST_SECRET).unwrap();
            let vault = VaultContext::generate().unwrap();
            let path = root.join("synthetic-credentials.json");
            let mut source = native_document(OAuthFamily::Zai, "a", "refreshed");
            let target = native
                .inspect(&native_document(OAuthFamily::Zai, "b", "initial"))
                .unwrap();
            let cache = target.identity().credential_keys()[5].clone();
            if cache_present {
                source = replace(&source, &cache, "old-b-cache-opaque".into());
            }
            let source_identity = native.inspect(&source).unwrap().identity().clone();
            let mut old_catalog = ProfileCatalog::default();
            old_catalog.upsert(
                native
                    .inspect(&native_document(OAuthFamily::Zai, "a", "stale"))
                    .unwrap(),
            );
            std::fs::write(
                root.join(PROFILE_FILE),
                old_catalog.seal(&vault, &native).unwrap(),
            )
            .unwrap();
            let mut checkpoint = SwitchCheckpoint::prepare(&source, &target, &native).unwrap();
            let changed = replace(
                &checkpoint.apply(&source).unwrap(),
                "ssh:unrelated",
                "newer-unrelated-value".into(),
            );
            std::fs::write(&path, changed.to_bytes().unwrap()).unwrap();
            checkpoint.set_phase(phase).unwrap();
            std::fs::write(
                root.join(JOURNAL_FILE),
                checkpoint.seal(&vault, &native).unwrap(),
            )
            .unwrap();
            drop(checkpoint);
            drop(old_catalog);

            let vault = reopen_vault(vault);
            let reopened = SwitchCheckpoint::open(
                &std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap(),
                &vault,
                &native,
            )
            .unwrap();
            let mut catalog = ProfileCatalog::open(
                &std::fs::read_to_string(root.join(PROFILE_FILE)).unwrap(),
                &vault,
                &native,
            )
            .unwrap();
            catalog.upsert(reopened.fresh_source().clone());
            std::fs::write(
                root.join(PROFILE_FILE),
                catalog.seal(&vault, &native).unwrap(),
            )
            .unwrap();
            match reopened
                .recover(&read_document(&path), JournalOrigin::Live)
                .unwrap()
            {
                RecoveryOutcome::Restore(restored) => {
                    std::fs::write(&path, restored.to_bytes().unwrap()).unwrap()
                }
                _ => panic!("expected exact preimage restore"),
            }
            let result = read_document(&path);
            for key in target.identity().credential_keys() {
                assert_eq!(result.get(&key), source.get(&key));
            }
            assert_eq!(result.get("ssh:unrelated"), Some("newer-unrelated-value"));
            for key in source_identity.credential_keys() {
                assert_eq!(
                    catalog
                        .get(&source_identity)
                        .unwrap()
                        .scoped_document()
                        .get(&key),
                    source.get(&key)
                );
            }
        }
    }
}

#[test]
fn encrypted_roundtrip_stale_import_commit_uncertainty_and_new_refresh_do_not_rewrite_live_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let native = NativeCipher::new(root.to_str().unwrap(), TEST_SECRET).unwrap();
    let vault = VaultContext::generate().unwrap();
    let source = native_document(OAuthFamily::Zai, "a", "fresh");
    let target_doc = native_document(OAuthFamily::Zai, "b", "initial");
    let target = native.inspect(&target_doc).unwrap();
    let path = root.join("synthetic-credentials.json");
    for phase in [
        TransactionPhase::Prepared,
        TransactionPhase::CommitUncertain,
        TransactionPhase::Committed,
    ] {
        let mut checkpoint = SwitchCheckpoint::prepare(&source, &target, &native).unwrap();
        let published = checkpoint.apply(&source).unwrap();
        std::fs::write(&path, published.to_bytes().unwrap()).unwrap();
        checkpoint.set_phase(phase).unwrap();
        let encoded = checkpoint.seal(&vault, &native).unwrap();
        std::fs::write(root.join(JOURNAL_FILE), &encoded).unwrap();
        drop(checkpoint);
        let reopened = SwitchCheckpoint::open(
            &std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap(),
            &vault,
            &native,
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            reopened.recover(&read_document(&path), JournalOrigin::Restored),
            Ok(RecoveryOutcome::Quarantine)
        ));
        match phase {
            TransactionPhase::CommitUncertain => assert!(matches!(
                reopened.recover(&read_document(&path), JournalOrigin::Live),
                Ok(RecoveryOutcome::ReconcileCommit)
            )),
            TransactionPhase::Committed => assert!(matches!(
                reopened.recover(&read_document(&path), JournalOrigin::Live),
                Ok(RecoveryOutcome::CleanupOnly)
            )),
            _ => {
                let newer = merge_synthetic_refresh(
                    &read_document(&path),
                    &native_document(OAuthFamily::Zai, "b", "newer"),
                    &native,
                );
                std::fs::write(&path, newer.to_bytes().unwrap()).unwrap();
                let before = std::fs::read(&path).unwrap();
                assert!(reopened
                    .recover(&read_document(&path), JournalOrigin::Live)
                    .is_err());
                assert_eq!(std::fs::read(&path).unwrap(), before);
                continue;
            }
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn encrypted_roundtrip_bad_envelope_and_wrong_bound_root_leave_files_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let root = directory.path();
    let native = NativeCipher::new(root.to_str().unwrap(), TEST_SECRET).unwrap();
    let vault = VaultContext::generate().unwrap();
    let source = native_document(OAuthFamily::BigModel, "a", "fresh");
    let target = native
        .inspect(&native_document(OAuthFamily::BigModel, "b", "fresh"))
        .unwrap();
    let checkpoint = SwitchCheckpoint::prepare(&source, &target, &native).unwrap();
    let encoded = checkpoint.seal(&vault, &native).unwrap();
    let path = root.join("synthetic-credentials.json");
    std::fs::write(&path, source.to_bytes().unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let wrong_context = NativeCipher::new(other.path().to_str().unwrap(), TEST_SECRET).unwrap();
    assert!(SwitchCheckpoint::open(&encoded, &vault, &wrong_context).is_err());
    let mut corrupt = encoded.into_bytes();
    let index = corrupt.len() / 2;
    corrupt[index] = if corrupt[index] == b'A' { b'B' } else { b'A' };
    assert!(
        SwitchCheckpoint::open(std::str::from_utf8(&corrupt).unwrap(), &vault, &native).is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
