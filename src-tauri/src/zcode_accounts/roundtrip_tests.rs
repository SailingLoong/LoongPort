//! Explicit temporary-directory composition tests, not native durability tests.
use super::checkpoint::{ProfileCatalog, SwitchCheckpoint, JOURNAL_FILE, PROFILE_FILE};
use super::core::{CredentialDocument, OAuthFamily, TransactionPhase};
use super::native::tests::{native_document, replace, TEST_SECRET};
use super::native::NativeCipher;
use super::recovery::{DispositionKind, JournalEvidence, RecoveryConfirmation, RecoveryLedger};
use crate::secrets::{owned_file::RECOVERY_FILE, VaultContext};
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
        let mut ledger = RecoveryLedger::default();
        ledger
            .record_completion(
                JournalEvidence::open_journal(
                    &std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap(),
                    &vault,
                )
                .unwrap(),
            )
            .unwrap();
        std::fs::write(root.join(RECOVERY_FILE), ledger.seal(&vault).unwrap()).unwrap();
        let saved = RecoveryLedger::open(
            &std::fs::read_to_string(root.join(RECOVERY_FILE)).unwrap(),
            &vault,
        )
        .unwrap();
        assert_eq!(
            saved.latest_completed().unwrap().disposition(),
            DispositionKind::FullAfter
        );
        std::fs::remove_file(root.join(JOURNAL_FILE)).unwrap();
        assert!(native.inspect(&read_document(&path)).unwrap().identity() == &b_identity);
        let refreshed_b = merge_synthetic_refresh(&read_document(&path), &b_prime, &native);
        std::fs::write(&path, refreshed_b.to_bytes().unwrap()).unwrap();
        drop(to_b);
        drop(catalog);
        vault = reopen_vault(vault);

        let completed = RecoveryLedger::open(
            &std::fs::read_to_string(root.join(RECOVERY_FILE)).unwrap(),
            &vault,
        )
        .unwrap();
        let committed = completed.latest_completed().unwrap().evidence();
        assert_eq!(committed.phase(), TransactionPhase::Committed);
        assert_eq!(
            committed
                .native_checkpoint(&native)
                .unwrap()
                .match_touched_image(&read_document(&path)),
            None
        );
        assert!(!completed.needs_confirmation());
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
fn encrypted_roundtrip_archive_preserves_old_catalog_and_both_native_publish_outcomes() {
    for phase in [
        TransactionPhase::Prepared,
        TransactionPhase::Captured,
        TransactionPhase::CredentialsPublished,
        TransactionPhase::CommitUncertain,
        TransactionPhase::Committed,
    ] {
        for cache_present in [false, true] {
            for published in [false, true] {
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
                let mut old_catalog = ProfileCatalog::default();
                old_catalog.upsert(
                    native
                        .inspect(&native_document(OAuthFamily::Zai, "a", "stale"))
                        .unwrap(),
                );
                let catalog_bytes = old_catalog.seal(&vault, &native).unwrap();
                std::fs::write(root.join(PROFILE_FILE), &catalog_bytes).unwrap();
                let mut checkpoint = SwitchCheckpoint::prepare(&source, &target, &native).unwrap();
                let image = if published {
                    checkpoint.apply(&source).unwrap()
                } else {
                    source.clone()
                };
                let changed = replace(&image, "ssh:unrelated", "newer-unrelated-value".into());
                let native_bytes = changed.to_bytes().unwrap();
                std::fs::write(&path, &native_bytes).unwrap();
                checkpoint.set_phase(phase).unwrap();
                std::fs::write(
                    root.join(JOURNAL_FILE),
                    checkpoint.seal(&vault, &native).unwrap(),
                )
                .unwrap();
                drop(checkpoint);
                drop(old_catalog);

                let vault = reopen_vault(vault);
                let journal = JournalEvidence::open_journal(
                    &std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap(),
                    &vault,
                )
                .unwrap();
                let raw = journal.raw_payload().to_vec();
                let id = journal.id().to_owned();
                let mut ledger = RecoveryLedger::default();
                ledger.archive(journal).unwrap();
                std::fs::write(root.join(RECOVERY_FILE), ledger.seal(&vault).unwrap()).unwrap();
                let mut reopened = RecoveryLedger::open(
                    &std::fs::read_to_string(root.join(RECOVERY_FILE)).unwrap(),
                    &vault,
                )
                .unwrap();
                assert!(reopened.needs_confirmation());
                let record = reopened.archived().next().unwrap();
                assert_eq!(record.evidence().raw_payload(), raw);
                assert_eq!(record.evidence().phase(), phase);
                std::fs::remove_file(root.join(JOURNAL_FILE)).unwrap();
                let confirmation = RecoveryConfirmation::from_image(
                    record.evidence(),
                    &read_document(&path),
                    &native,
                )
                .unwrap();
                reopened.confirm(&id, confirmation).unwrap();
                let expected = if published {
                    DispositionKind::FullAfter
                } else {
                    DispositionKind::FullBefore
                };
                assert_eq!(reopened.archived().next().unwrap().disposition(), expected);
                std::fs::write(root.join(RECOVERY_FILE), reopened.seal(&vault).unwrap()).unwrap();
                let confirmed = RecoveryLedger::open(
                    &std::fs::read_to_string(root.join(RECOVERY_FILE)).unwrap(),
                    &vault,
                )
                .unwrap();
                assert!(!confirmed.needs_confirmation());
                assert_eq!(
                    confirmed
                        .archived()
                        .next()
                        .unwrap()
                        .evidence()
                        .raw_payload(),
                    raw
                );
                assert_eq!(std::fs::read(&path).unwrap(), native_bytes);
                assert_eq!(
                    std::fs::read_to_string(root.join(PROFILE_FILE)).unwrap(),
                    catalog_bytes
                );
                assert_eq!(
                    read_document(&path).get("ssh:unrelated"),
                    Some("newer-unrelated-value")
                );
            }
        }
    }
}

#[test]
fn encrypted_roundtrip_all_archived_phases_leave_mixed_refreshed_and_third_account_files_unchanged()
{
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
        TransactionPhase::Captured,
        TransactionPhase::CredentialsPublished,
        TransactionPhase::CommitUncertain,
        TransactionPhase::Committed,
    ] {
        let mut checkpoint = SwitchCheckpoint::prepare(&source, &target, &native).unwrap();
        let published = checkpoint.apply(&source).unwrap();
        checkpoint.set_phase(phase).unwrap();
        let encoded = checkpoint.seal(&vault, &native).unwrap();
        std::fs::write(root.join(JOURNAL_FILE), &encoded).unwrap();
        let access_key = &target.identity().credential_keys()[1];
        let mixed = replace(
            &published,
            access_key,
            source.get(access_key).unwrap().into(),
        );
        assert!(native.inspect(&mixed).unwrap().identity() == target.identity());
        let newer = merge_synthetic_refresh(
            &published,
            &native_document(OAuthFamily::Zai, "b", "newer"),
            &native,
        );
        let third = native_document(OAuthFamily::Zai, "c", "official-login");
        for image in [mixed, newer, third] {
            let image_bytes = image.to_bytes().unwrap();
            std::fs::write(&path, &image_bytes).unwrap();
            let evidence = JournalEvidence::open_journal(
                &std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap(),
                &vault,
            )
            .unwrap();
            let raw = evidence.raw_payload().to_vec();
            let mut ledger = RecoveryLedger::default();
            ledger.archive(evidence).unwrap();
            std::fs::write(root.join(RECOVERY_FILE), ledger.seal(&vault).unwrap()).unwrap();
            let archived = RecoveryLedger::open(
                &std::fs::read_to_string(root.join(RECOVERY_FILE)).unwrap(),
                &vault,
            )
            .unwrap();
            let record = archived.archived().next().unwrap();
            assert_eq!(record.evidence().raw_payload(), raw);
            assert!(RecoveryConfirmation::from_image(
                record.evidence(),
                &read_document(&path),
                &native
            )
            .is_err());
            assert!(archived.needs_confirmation());
            assert_eq!(std::fs::read(&path).unwrap(), image_bytes);
            assert_eq!(
                std::fs::read_to_string(root.join(JOURNAL_FILE)).unwrap(),
                encoded
            );
        }
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
