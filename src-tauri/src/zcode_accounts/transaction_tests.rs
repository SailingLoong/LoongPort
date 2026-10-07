use super::super::checkpoint::{JOURNAL_FILE, PROFILE_FILE};
use super::super::core::{CredentialDocument, OAuthFamily};
use super::super::native::tests::{native_document, replace, TEST_CONTEXT, TEST_SECRET};
use super::*;
use crate::config_file_io::write_durable;
use std::fs;

const ADMITTED: Admission = Admission {
    contract_verified: true,
    app_stopped: true,
    native_gate_passed: true,
    individual_scope_verified: true,
};
struct Fixture {
    native_root: tempfile::TempDir,
    vault_root: tempfile::TempDir,
    vault: VaultContext,
    native: NativeCipher,
    a: AccountIdentity,
    b: AccountIdentity,
    fresh: CredentialDocument,
}
impl Fixture {
    fn new(family: OAuthFamily) -> Self {
        let temp = fs::canonicalize(std::env::temp_dir()).unwrap();
        let native_root = tempfile::tempdir_in(&temp).unwrap();
        let vault_root = tempfile::tempdir_in(&temp).unwrap();
        crate::config_file_io::ensure_private_directory(native_root.path()).unwrap();
        crate::config_file_io::ensure_private_directory(vault_root.path()).unwrap();
        let vault = VaultContext::generate().unwrap();
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        let old = native_document(family, "a", "old");
        let fresh = native_document(family, "a", "fresh");
        let target = native_document(family, "b", "saved");
        let a = native.inspect(&old).unwrap().identity().clone();
        let b = native.inspect(&target).unwrap().identity().clone();
        let mut catalog = ProfileCatalog::default();
        catalog.upsert(native.inspect(&old).unwrap());
        catalog.upsert(native.inspect(&target).unwrap());
        write_durable(
            &native_root.path().join("credentials.json"),
            &fresh.to_bytes().unwrap(),
        )
        .unwrap();
        write_durable(
            &vault_root.path().join(PROFILE_FILE),
            catalog.seal(&vault, &native).unwrap().as_bytes(),
        )
        .unwrap();
        fs::write(native_root.path().join("telemetry.json"), "unchanged").unwrap();
        Self {
            native_root,
            vault_root,
            vault,
            native,
            a,
            b,
            fresh,
        }
    }
    fn store(&self) -> AccountStore<'_> {
        AccountStore::new(
            self.native_root.path(),
            self.vault_root.path(),
            &self.vault,
            &self.native,
            ADMITTED,
        )
        .unwrap()
    }
    fn current(&self) -> CredentialDocument {
        CredentialDocument::parse(
            &fs::read(self.native_root.path().join("credentials.json")).unwrap(),
        )
        .unwrap()
    }
    fn catalog(&self) -> ProfileCatalog {
        ProfileCatalog::open(
            &fs::read_to_string(self.vault_root.path().join(PROFILE_FILE)).unwrap(),
            &self.vault,
            &self.native,
        )
        .unwrap()
    }
}
fn archive_and_confirm(f: &Fixture, store: &AccountStore<'_>) -> ArchiveOutcome {
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let outcome = local
        .archive_pending(&local.status().unwrap().revision)
        .unwrap();
    let status = local.status().unwrap();
    for record in status.records {
        if record.disposition == "native-unconfirmed" {
            store
                .confirm_archived(&record.id, &local.status().unwrap().revision)
                .unwrap();
        }
    }
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        before
    );
    assert!(!local.status().unwrap().native_unconfirmed);
    outcome
}

#[test]
fn io_key_intent_is_persisted_before_grant_and_restart_is_recover_only() {
    let f = Fixture::new(OAuthFamily::BigModel);
    let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let scope = KeyScope::new(
        OAuthFamily::BigModel,
        "synthetic-account",
        "synthetic-org",
        "synthetic-project",
    )
    .unwrap();
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let Reservation::Fresh(grant) = store.reserve_key_intent(scope.clone()).unwrap() else {
        panic!("first reservation")
    };
    let path = f
        .vault_root
        .path()
        .join(crate::secrets::owned_file::KEY_INTENT_FILE);
    let sealed = fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("synthetic-account"));
    let reopened = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    assert!(matches!(
        reopened.reserve_key_intent(scope.clone()).unwrap(),
        Reservation::Existing(_)
    ));
    assert_eq!(
        fs::read(&path).unwrap(),
        sealed,
        "recovery lookup must not rewrite intent"
    );
    reopened.mark_key_created(&grant).unwrap();
    assert!(reopened.clear_unsubmitted_key(&grant).is_err());
    let record = reopened.key_intent(&scope).unwrap().unwrap();
    reopened.clear_resolved_key(&record).unwrap();
    assert!(!path.exists());
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn io_key_intent_refuses_existing_symlink_without_grant_or_target_change() {
    let f = Fixture::new(OAuthFamily::BigModel);
    let outside = tempfile::NamedTempFile::new().unwrap();
    let scope = KeyScope::new(
        OAuthFamily::BigModel,
        "synthetic-account",
        "synthetic-org",
        "synthetic-project",
    )
    .unwrap();
    let mut ledger = KeyIntentLedger::default();
    ledger.reserve(scope.clone()).unwrap();
    let sealed = ledger.seal(&f.vault).unwrap();
    fs::write(outside.path(), &sealed).unwrap();
    std::os::unix::fs::symlink(
        outside.path(),
        f.vault_root
            .path()
            .join(crate::secrets::owned_file::KEY_INTENT_FILE),
    )
    .unwrap();
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    assert!(store.reserve_key_intent(scope).is_err());
    assert_eq!(fs::read(outside.path()).unwrap(), sealed);
}

#[test]
fn io_key_intent_no_post_cleanup_removes_marker_and_allows_maintenance() {
    let f = Fixture::new(OAuthFamily::BigModel);
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let scope = KeyScope::new(
        OAuthFamily::BigModel,
        "synthetic-account",
        "synthetic-org",
        "synthetic-project",
    )
    .unwrap();
    let Reservation::Fresh(grant) = store.reserve_key_intent(scope).unwrap() else {
        panic!("first reservation")
    };
    assert!(
        crate::secrets::owned_file::ensure_no_pending_zcode_transaction(f.vault_root.path())
            .is_err()
    );
    store.clear_unsubmitted_key(&grant).unwrap();
    crate::secrets::owned_file::ensure_no_pending_zcode_transaction(f.vault_root.path()).unwrap();
}
#[test]
fn io_unverified_import_cannot_activate_or_change_native_journal_or_source() {
    let f = Fixture::new(OAuthFamily::Zai);
    let mut catalog = f.catalog();
    catalog.upsert_unverified(
        f.native
            .inspect(&native_document(OAuthFamily::Zai, "b", "imported"))
            .unwrap(),
    );
    write_durable(
        &f.vault_root.path().join(PROFILE_FILE),
        catalog.seal(&f.vault, &f.native).unwrap().as_bytes(),
    )
    .unwrap();
    let before_native = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let before_catalog = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    assert_eq!(
        f.store()
            .switch_saved(&f.b.opaque_id(), &saved_revision(&f), OAuthFamily::Zai),
        Err(TransactionError::UnverifiedSource)
    );
    assert_eq!(
        f.store().switch(&f.b),
        Err(TransactionError::UnverifiedSource)
    );
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        before_native
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        before_catalog
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
}

#[test]
fn io_bundle_vault_import_preserves_native_and_keeps_unchecked_updates_to_valid_records() {
    let f = Fixture::new(OAuthFamily::Zai);
    let native_before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let native_files_before = fs::read_dir(f.native_root.path())
        .unwrap()
        .map(|p| p.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let make = |id| {
        f.native
            .inspect(&native_document(OAuthFamily::Zai, id, "imported"))
            .unwrap()
    };
    assert_eq!(
        store.import_profiles(
            &f.native,
            &saved_revision(&f),
            vec![(make("a"), false), (make("c"), false)]
        ),
        Ok(vec![
            CaptureCommitOutcome::Kept,
            CaptureCommitOutcome::Saved
        ])
    );
    assert!(f.catalog().source_verified(&f.a));
    let c = make("c").identity().clone();
    assert!(!f.catalog().source_verified(&c));
    assert_eq!(
        store.import_profiles(&f.native, &saved_revision(&f), vec![(make("a"), true)]),
        Ok(vec![CaptureCommitOutcome::Kept])
    );
    assert!(f.catalog().source_verified(&f.a));
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        native_before
    );
    assert_eq!(
        fs::read_dir(f.native_root.path())
            .unwrap()
            .map(|p| p.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>(),
        native_files_before
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
}

#[test]
fn io_bundle_vault_import_checks_revision_duplicates_and_whole_batch_before_publication() {
    let f = Fixture::new(OAuthFamily::Zai);
    // A vault-only import does not need to open the native credential file.
    fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let make = || {
        f.native
            .inspect(&native_document(OAuthFamily::Zai, "c", "imported"))
            .unwrap()
    };
    let before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    assert_eq!(
        store.import_profiles(&f.native, "stale", vec![(make(), false)]),
        Err(TransactionError::CatalogChanged)
    );
    assert!(store
        .import_profiles(
            &f.native,
            &saved_revision(&f),
            vec![(make(), false), (make(), true)]
        )
        .is_err());
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        before
    );
    assert_eq!(
        store.import_profiles(&f.native, &saved_revision(&f), vec![(make(), false)]),
        Ok(vec![CaptureCommitOutcome::Saved])
    );
    assert!(!f.native_root.path().join("credentials.json").exists());
}

#[test]
fn io_unverified_duplicate_keep_remains_blocked_explicit_local_update_enables_it() {
    let f = Fixture::new(OAuthFamily::Zai);
    let mut catalog = f.catalog();
    catalog.upsert_unverified(
        f.native
            .inspect(&native_document(OAuthFamily::Zai, "a", "imported"))
            .unwrap(),
    );
    write_durable(
        &f.vault_root.path().join(PROFILE_FILE),
        catalog.seal(&f.vault, &f.native).unwrap().as_bytes(),
    )
    .unwrap();
    let store = f.store();
    let preview = store
        .preview_capture(OAuthFamily::Zai, &saved_revision(&f))
        .unwrap();
    assert_eq!(
        store.capture_reviewed(
            OAuthFamily::Zai,
            &saved_revision(&f),
            &preview.native_revision,
            &preview.id,
            false
        ),
        Ok(CaptureCommitOutcome::Kept)
    );
    assert!(!f.catalog().source_verified(&f.a));
    assert_eq!(
        store.capture_reviewed(
            OAuthFamily::Zai,
            &saved_revision(&f),
            &preview.native_revision,
            &preview.id,
            true
        ),
        Ok(CaptureCommitOutcome::Refreshed)
    );
    assert!(f.catalog().source_verified(&f.a));
    assert!(
        f.catalog().get(&f.a).unwrap().scoped_document()
            == f.native.inspect(&f.fresh).unwrap().scoped_document()
    );
}
#[test]
fn io_switch_refreshes_outgoing_a_then_restores_fresh_a_for_each_family() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let f = Fixture::new(family);
        assert_eq!(f.store().switch(&f.b), Ok(SwitchOutcome::Switched));
        assert!(f.native.inspect(&f.current()).unwrap().identity() == &f.b);
        assert!(
            f.catalog().get(&f.a).unwrap().scoped_document()
                == f.native.inspect(&f.fresh).unwrap().scoped_document()
        );
        assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
        assert_eq!(f.store().switch(&f.a), Ok(SwitchOutcome::Switched));
        for key in f.a.credential_keys() {
            assert_eq!(f.current().get(&key), f.fresh.get(&key));
        }
        assert_eq!(
            fs::read_to_string(f.native_root.path().join("telemetry.json")).unwrap(),
            "unchanged"
        );
    }
}
#[test]
fn io_same_identity_refreshes_catalog_without_replacing_native_bytes() {
    let f = Fixture::new(OAuthFamily::Zai);
    let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    assert_eq!(f.store().switch(&f.a), Ok(SwitchOutcome::Refreshed));
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        before
    );
    assert!(
        f.catalog().get(&f.a).unwrap().scoped_document()
            == f.native.inspect(&f.fresh).unwrap().scoped_document()
    );
}
#[test]
fn io_unadmitted_or_unsaved_source_never_writes() {
    let f = Fixture::new(OAuthFamily::Zai);
    let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    for admission in [
        Admission {
            contract_verified: false,
            ..ADMITTED
        },
        Admission {
            app_stopped: false,
            ..ADMITTED
        },
        Admission {
            native_gate_passed: false,
            ..ADMITTED
        },
    ] {
        let result = AccountStore::new(
            f.native_root.path(),
            f.vault_root.path(),
            &f.vault,
            &f.native,
            admission,
        )
        .and_then(|store| store.switch(&f.b));
        assert_eq!(result, Err(TransactionError::NotAdmitted));
    }
    let mut catalog = ProfileCatalog::default();
    catalog.upsert(
        f.native
            .inspect(&native_document(OAuthFamily::Zai, "b", "saved"))
            .unwrap(),
    );
    write_durable(
        &f.vault_root.path().join(PROFILE_FILE),
        catalog.seal(&f.vault, &f.native).unwrap().as_bytes(),
    )
    .unwrap();
    assert_eq!(
        f.store().switch(&f.b),
        Err(TransactionError::MissingSavedSource)
    );
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        before
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
}
#[test]
fn io_precommit_failure_keeps_target_image_and_unrelated_changes() {
    let f = Fixture::new(OAuthFamily::Zai);
    assert!(f
        .store()
        .switch_with_hook(&f.b, &mut |p| {
            if p == WritePoint::NativeCredentials {
                Err(TransactionError::Storage)
            } else {
                Ok(())
            }
        })
        .is_err());
    let altered = replace(&f.current(), "future:unrelated", "after-failure".into());
    write_durable(
        &f.native_root.path().join("credentials.json"),
        &altered.to_bytes().unwrap(),
    )
    .unwrap();
    assert_eq!(
        archive_and_confirm(&f, &f.store()),
        ArchiveOutcome::Archived
    );
    assert!(f.current() == altered);
    assert!(f.native.inspect(&f.current()).unwrap().identity() == &f.b);
}

#[test]
fn io_committed_journal_is_archived_without_native_write() {
    let f = Fixture::new(OAuthFamily::Zai);
    assert!(f
        .store()
        .switch_with_hook(&f.b, &mut |p| {
            if p == WritePoint::Committed {
                Err(TransactionError::Storage)
            } else {
                Ok(())
            }
        })
        .is_err());
    let current = f.current();
    assert_eq!(
        archive_and_confirm(&f, &f.store()),
        ArchiveOutcome::Archived
    );
    assert!(f.current() == current);
    assert_eq!(
        archive_and_confirm(&f, &f.store()),
        ArchiveOutcome::NothingPending
    );
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ChildInput {
    native_root: std::path::PathBuf,
    vault_root: std::path::PathBuf,
    metadata: crate::secrets::VaultMetadata,
    key: Vec<u8>,
    checkpoint: usize,
}
const POINTS: [WritePoint; 7] = [
    WritePoint::Prepared,
    WritePoint::OutgoingProfile,
    WritePoint::Captured,
    WritePoint::NativeCredentials,
    WritePoint::Committed,
    WritePoint::RecoveryRecord,
    WritePoint::JournalRemoved,
];
#[test]
#[ignore = "invoked only by the synthetic subprocess crash matrix"]
fn synthetic_account_crash_child() {
    use std::io::Read;
    assert_eq!(
        std::env::var("LOONGPORT_ZCODE_SYNTHETIC_CHILD").as_deref(),
        Ok("1")
    );
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    std::io::stdin()
        .take(16384)
        .read_to_end(&mut bytes)
        .unwrap();
    let input: ChildInput = serde_json::from_slice(&bytes).unwrap();
    let vault = VaultContext::from_key(input.metadata, zeroize::Zeroizing::new(input.key)).unwrap();
    let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
    let target = AccountIdentity::new(TEST_CONTEXT, OAuthFamily::Zai, "b").unwrap();
    let store = AccountStore::new(
        &input.native_root,
        &input.vault_root,
        &vault,
        &native,
        ADMITTED,
    )
    .unwrap();
    store
        .switch_with_hook(&target, &mut |point| {
            if point == POINTS[input.checkpoint] {
                std::process::exit(73);
            }
            Ok(())
        })
        .unwrap();
    panic!("requested crash point was not reached");
}
#[test]
fn io_actual_process_crash_at_every_publication_recovers_from_disk() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (index, point) in POINTS.into_iter().enumerate() {
        let f = Fixture::new(OAuthFamily::Zai);
        let input = ChildInput {
            native_root: f.native_root.path().into(),
            vault_root: f.vault_root.path().into(),
            metadata: f.vault.metadata().clone(),
            key: f.vault.export_key().to_vec(),
            checkpoint: index,
        };
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "zcode_accounts::transaction::tests::synthetic_account_crash_child",
                "--nocapture",
            ])
            .env("LOONGPORT_ZCODE_SYNTHETIC_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&input).unwrap());
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(73),
            "crash point {point:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // A newly created cipher and vault context re-open actual child-written
        // bytes. Only synthetic key custody is retained in parent memory.
        let vault =
            VaultContext::from_key(f.vault.metadata().clone(), f.vault.export_key()).unwrap();
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        let reopened = AccountStore::new(
            f.native_root.path(),
            f.vault_root.path(),
            &vault,
            &native,
            ADMITTED,
        )
        .unwrap();
        let result = archive_and_confirm(&f, &reopened);
        assert_eq!(
            result,
            if point == WritePoint::JournalRemoved {
                ArchiveOutcome::NothingPending
            } else {
                ArchiveOutcome::Archived
            }
        );
        let expected = if index < 3 { &f.a } else { &f.b };
        assert!(
            f.native.inspect(&f.current()).unwrap().identity() == expected,
            "crash point {point:?}"
        );
        if point != WritePoint::Prepared {
            assert!(
                f.catalog().get(&f.a).unwrap().scoped_document()
                    == f.native.inspect(&f.fresh).unwrap().scoped_document()
            );
        }
        let local = VaultAccountStore::new(f.vault_root.path(), &vault).unwrap();
        let ledger = local
            .open_ledger(Some(
                &fs::read(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
            ))
            .unwrap();
        assert!(ledger
            .archived()
            .chain(ledger.latest_completed())
            .any(|record| {
                record
                    .evidence()
                    .native_checkpoint(&native)
                    .unwrap()
                    .fresh_source()
                    .scoped_document()
                    == f.native.inspect(&f.fresh).unwrap().scoped_document()
            }));
        assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
        assert_eq!(
            fs::read_to_string(f.native_root.path().join("telemetry.json")).unwrap(),
            "unchanged"
        );
        for name in [PROFILE_FILE, crate::secrets::owned_file::RECOVERY_FILE] {
            let bytes = fs::read(f.vault_root.path().join(name)).unwrap();
            assert!(bytes.starts_with(b"lpenc"));
            assert!(!String::from_utf8_lossy(&bytes).contains("SYNTHETIC_CANARY"));
        }
    }
}

#[test]
fn io_each_error_boundary_keeps_recoverable_state() {
    for point in POINTS {
        let f = Fixture::new(OAuthFamily::Zai);
        assert!(f
            .store()
            .switch_with_hook(&f.b, &mut |current| if current == point {
                Err(TransactionError::Storage)
            } else {
                Ok(())
            })
            .is_err());
        let outcome = archive_and_confirm(&f, &f.store());
        assert_eq!(
            outcome,
            if point == WritePoint::JournalRemoved {
                ArchiveOutcome::NothingPending
            } else {
                ArchiveOutcome::Archived
            }
        );
        let committed = matches!(
            point,
            WritePoint::NativeCredentials
                | WritePoint::Committed
                | WritePoint::RecoveryRecord
                | WritePoint::JournalRemoved
        );
        assert!(
            f.native.inspect(&f.current()).unwrap().identity()
                == if committed { &f.b } else { &f.a }
        );
    }
}
#[test]
fn io_archive_keeps_newer_profiles_and_unknown_native_values() {
    for change_profile in [false, true] {
        let f = Fixture::new(OAuthFamily::Zai);
        assert!(f
            .store()
            .switch_with_hook(&f.b, &mut |p| {
                if p == WritePoint::NativeCredentials {
                    Err(TransactionError::Storage)
                } else {
                    Ok(())
                }
            })
            .is_err());
        if change_profile {
            let mut catalog = f.catalog();
            catalog.upsert(
                f.native
                    .inspect(&native_document(OAuthFamily::Zai, "a", "even-newer"))
                    .unwrap(),
            );
            write_durable(
                &f.vault_root.path().join(PROFILE_FILE),
                catalog.seal(&f.vault, &f.native).unwrap().as_bytes(),
            )
            .unwrap();
        } else {
            let current = replace(
                &f.current(),
                "oauth:zai:access_token",
                "unknown-later-value".into(),
            );
            write_durable(
                &f.native_root.path().join("credentials.json"),
                &current.to_bytes().unwrap(),
            )
            .unwrap();
        }
        let current = fs::read(f.native_root.path().join("credentials.json")).unwrap();
        let profiles = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
        let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
        local
            .archive_pending(&local.status().unwrap().revision)
            .unwrap();
        let status = local.status().unwrap();
        let result = f
            .store()
            .confirm_archived(&status.records[0].id, &status.revision);
        if change_profile {
            assert_eq!(result, Ok(()));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(
            fs::read(f.native_root.path().join("credentials.json")).unwrap(),
            current
        );
        assert_eq!(
            fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
            profiles
        );
        assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
        assert_eq!(local.status().unwrap().native_unconfirmed, !change_profile);
    }
}

#[test]
fn io_native_revision_change_before_publication_does_not_overwrite_other_writer() {
    let f = Fixture::new(OAuthFamily::Zai);
    let altered = replace(&f.current(), "future:other-writer", "preserve".into())
        .to_bytes()
        .unwrap();
    let result = f.store().switch_with_hook(&f.b, &mut |p| {
        if p == WritePoint::Captured {
            write_durable(&f.native_root.path().join("credentials.json"), &altered).unwrap();
        }
        Ok(())
    });
    assert_eq!(result, Err(TransactionError::SourceChanged));
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        altered
    );
    archive_and_confirm(&f, &f.store());
    assert_eq!(f.current().get("future:other-writer"), Some("preserve"));
}
#[cfg(unix)]
#[test]
fn io_rejects_links_unsafe_modes_and_replaced_roots_without_writing() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    for kind in ["symlink", "hardlink", "mode"] {
        let f = Fixture::new(OAuthFamily::Zai);
        let file = f.native_root.path().join("credentials.json");
        let other = f.native_root.path().join("original.json");
        let before = fs::read(&file).unwrap();
        match kind {
            "symlink" => {
                fs::rename(&file, &other).unwrap();
                symlink(&other, &file).unwrap();
            }
            "hardlink" => fs::hard_link(&file, &other).unwrap(),
            _ => fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap(),
        }
        assert_eq!(f.store().switch(&f.b), Err(TransactionError::UnsafePath));
        assert_eq!(fs::read(&file).unwrap(), before);
        assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    }
    let f = Fixture::new(OAuthFamily::Zai);
    let store = f.store();
    assert_eq!(
        store.native_root_identity(),
        root_identity(f.native_root.path()).unwrap()
    );
    let moved = f.native_root.path().with_extension("moved");
    fs::rename(f.native_root.path(), &moved).unwrap();
    fs::create_dir(f.native_root.path()).unwrap();
    crate::config_file_io::ensure_private_directory(f.native_root.path()).unwrap();
    assert_eq!(store.switch(&f.b), Err(TransactionError::UnsafePath));
    fs::remove_dir(f.native_root.path()).unwrap();
    fs::rename(moved, f.native_root.path()).unwrap();
}

#[test]
fn io_team_unknown_scope_and_cross_family_are_explicitly_rejected() {
    let f = Fixture::new(OAuthFamily::Zai);
    let admission = Admission {
        individual_scope_verified: false,
        ..ADMITTED
    };
    let result = AccountStore::new(
        f.native_root.path(),
        f.vault_root.path(),
        &f.vault,
        &f.native,
        admission,
    )
    .and_then(|store| store.switch(&f.b));
    assert_eq!(result, Err(TransactionError::UnsupportedScope));
    let mut catalog = f.catalog();
    let other = f
        .native
        .inspect(&native_document(OAuthFamily::BigModel, "c", "saved"))
        .unwrap();
    let identity = other.identity().clone();
    catalog.upsert(other);
    write_durable(
        &f.vault_root.path().join(PROFILE_FILE),
        catalog.seal(&f.vault, &f.native).unwrap().as_bytes(),
    )
    .unwrap();
    assert_eq!(
        f.store().switch(&identity),
        Err(TransactionError::Checkpoint(CheckpointError::Core(
            super::super::core::CoreError::DifferentFamily
        )))
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    assert!(f.current() == f.fresh);
}

#[test]
fn io_same_bytes_replacement_of_any_observed_file_fails_identity_cas() {
    use std::os::unix::fs::MetadataExt;
    for (role, point) in [
        (Role::Credentials, WritePoint::Captured),
        (Role::Profiles, WritePoint::Prepared),
        (Role::Journal, WritePoint::OutgoingProfile),
        (Role::Recovery, WritePoint::Committed),
    ] {
        let f = Fixture::new(OAuthFamily::Zai);
        // Make a legitimate previous recovery record as well as both profiles.
        f.store().switch(&f.b).unwrap();
        f.store().switch(&f.a).unwrap();
        let path = f.store().root(role).join(role.name());
        let result = f.store().switch_with_hook(&f.b, &mut |current| {
            if current == point {
                let before = fs::read(&path).unwrap();
                let handle = fs::File::open(&path).unwrap();
                let inode = handle.metadata().unwrap().ino();
                write_durable(&path, &before).unwrap();
                assert_ne!(fs::metadata(&path).unwrap().ino(), inode);
            }
            Ok(())
        });
        // Replacing the recovery record is detected after the durable commit;
        // callers must retain that outcome while they reconcile the CAS failure.
        assert_eq!(
            result,
            Err(if point == WritePoint::Committed {
                TransactionError::CommittedNeedsCleanup
            } else {
                TransactionError::SourceChanged
            })
        );
        assert!(f.vault_root.path().join(JOURNAL_FILE).exists());
        // Recovery is a new operation: it validates current identities, rather
        // than insisting on the inode we replaced before the crash/error.
        archive_and_confirm(&f, &f.store());
    }
}

#[test]
fn io_publication_gate_blocks_restarted_writer_without_native_write() {
    let f = Fixture::new(OAuthFamily::Zai);
    let running = std::cell::Cell::new(false);
    let gate = || {
        if running.get() {
            Err(BlockedReason::AppRunning)
        } else {
            Ok(())
        }
    };
    let store = AccountStore::new_guarded(
        f.native_root.path(),
        f.vault_root.path(),
        &f.vault,
        &f.native,
        ADMITTED,
        &gate,
    )
    .unwrap();
    let result = store.switch_with_hook(&f.b, &mut |point| {
        if point == WritePoint::Captured {
            running.set(true);
        }
        Ok(())
    });
    assert_eq!(
        result,
        Err(TransactionError::Admission(BlockedReason::AppRunning))
    );
    assert!(f.current() == f.fresh);
    assert!(f.vault_root.path().join(JOURNAL_FILE).exists());
    running.set(false);
    archive_and_confirm(&f, &store);
}
#[test]
fn io_postcommit_gate_failure_keeps_committed_marker_and_target() {
    let f = Fixture::new(OAuthFamily::Zai);
    let changed = std::cell::Cell::new(false);
    let gate = || {
        if changed.get() {
            Err(BlockedReason::ContextChanged)
        } else {
            Ok(())
        }
    };
    let store = AccountStore::new_guarded(
        f.native_root.path(),
        f.vault_root.path(),
        &f.vault,
        &f.native,
        ADMITTED,
        &gate,
    )
    .unwrap();
    let result = store.switch_with_hook(&f.b, &mut |point| {
        if point == WritePoint::Committed {
            changed.set(true);
        }
        Ok(())
    });
    assert_eq!(result, Err(TransactionError::CommittedNeedsCleanup));
    let current = f.current();
    assert!(f.native.inspect(&current).unwrap().identity() == &f.b);
    changed.set(false);
    archive_and_confirm(&f, &store);
    assert!(f.current() == current);
}
#[test]
fn io_archive_cleanup_error_retains_authenticated_record_and_native_image() {
    let f = Fixture::new(OAuthFamily::Zai);
    assert_eq!(
        f.store().switch_with_hook(&f.b, &mut |point| {
            if point == WritePoint::Committed {
                Err(TransactionError::Storage)
            } else {
                Ok(())
            }
        }),
        Err(TransactionError::CommittedNeedsCleanup)
    );
    let current = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    assert_eq!(
        local.archive_with_hook(&local.status().unwrap().revision, &mut |point| {
            if point == ArchivePoint::JournalRemoved {
                Err(TransactionError::Storage)
            } else {
                Ok(())
            }
        }),
        Err(TransactionError::ArchiveNeedsCleanup)
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    assert!(local.status().unwrap().native_unconfirmed);
    assert_eq!(
        local.archive_pending(&local.status().unwrap().revision),
        Ok(ArchiveOutcome::NothingPending)
    );
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        current
    );
}

#[test]
fn io_visible_committed_marker_reconciliation_does_not_read_native_credentials() {
    let f = Fixture::new(OAuthFamily::Zai);
    let result = f.store().switch_with_hook(&f.b, &mut |point| {
        if point == WritePoint::Committed {
            fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
            return Err(TransactionError::Storage);
        }
        Ok(())
    });
    assert_eq!(result, Err(TransactionError::CommittedNeedsCleanup));
    assert!(!f.native_root.path().join("credentials.json").exists());
    assert!(f.vault_root.path().join(JOURNAL_FILE).exists());
}

fn saved_revision(f: &Fixture) -> String {
    use base64::Engine;
    format!(
        "sha256:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash(
            &fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap()
        ))
    )
}

#[test]
fn io_reviewed_capture_previews_without_saving_and_keep_does_not_overwrite() {
    let f = Fixture::new(OAuthFamily::Zai);
    let revision = saved_revision(&f);
    let native_before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let saved_before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let preview = f
        .store()
        .preview_capture(OAuthFamily::Zai, &revision)
        .unwrap();
    assert!(preview.duplicate);
    assert_eq!(preview.id, f.a.opaque_id());
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        saved_before
    );
    let encoded = serde_json::to_string(&preview).unwrap();
    for forbidden in ["SYNTHETIC_CANARY", "enc:v1:", "access_token", "user_info"] {
        assert!(!encoded.contains(forbidden));
    }
    assert_eq!(
        f.store().capture_reviewed(
            OAuthFamily::Zai,
            &revision,
            &preview.native_revision,
            &preview.id,
            false
        ),
        Ok(CaptureCommitOutcome::Kept)
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        saved_before
    );
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        native_before
    );
}

#[test]
fn io_reviewed_capture_rejects_wrong_identity_with_unchanged_native_bytes() {
    let f = Fixture::new(OAuthFamily::Zai);
    let revision = saved_revision(&f);
    let preview = f
        .store()
        .preview_capture(OAuthFamily::Zai, &revision)
        .unwrap();
    let before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let native = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    assert_eq!(
        f.store().capture_reviewed(
            OAuthFamily::Zai,
            &revision,
            &preview.native_revision,
            "wrong-identity",
            true
        ),
        Err(TransactionError::SourceChanged)
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        before
    );
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        native
    );
}

#[test]
fn io_reviewed_capture_explicit_update_refreshes_one_identity_without_native_write() {
    let f = Fixture::new(OAuthFamily::BigModel);
    let revision = saved_revision(&f);
    let native_before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let preview = f
        .store()
        .preview_capture(OAuthFamily::BigModel, &revision)
        .unwrap();
    assert_eq!(
        f.store().capture_reviewed(
            OAuthFamily::BigModel,
            &revision,
            &preview.native_revision,
            &preview.id,
            true
        ),
        Ok(CaptureCommitOutcome::Refreshed)
    );
    assert_eq!(f.catalog().len(), 2);
    assert!(
        f.catalog().get(&f.a).unwrap().scoped_document()
            == f.native.inspect(&f.fresh).unwrap().scoped_document()
    );
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        native_before
    );
}

#[test]
fn io_reviewed_capture_rejects_drift_before_saving_and_new_identity_can_be_saved() {
    let f = Fixture::new(OAuthFamily::Zai);
    fs::remove_file(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let preview = f
        .store()
        .preview_capture(OAuthFamily::Zai, "absent")
        .unwrap();
    assert!(!preview.duplicate);
    let different = native_document(OAuthFamily::Zai, "different", "fresh");
    write_durable(
        &f.native_root.path().join("credentials.json"),
        &different.to_bytes().unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.store().capture_reviewed(
            OAuthFamily::Zai,
            "absent",
            &preview.native_revision,
            &preview.id,
            false
        ),
        Err(TransactionError::SourceChanged)
    );
    assert!(!f.vault_root.path().join(PROFILE_FILE).exists());
    let current = f
        .store()
        .preview_capture(OAuthFamily::Zai, "absent")
        .unwrap();
    assert_eq!(
        f.store().capture_reviewed(
            OAuthFamily::Zai,
            "absent",
            &current.native_revision,
            &current.id,
            false
        ),
        Ok(CaptureCommitOutcome::Saved)
    );
    assert_eq!(f.catalog().len(), 1);
    assert_eq!(
        f.store().capture_reviewed(
            OAuthFamily::Zai,
            "absent",
            &current.native_revision,
            &current.id,
            false
        ),
        Err(TransactionError::CatalogChanged)
    );
}
#[test]
fn io_explicit_capture_creates_one_encrypted_profile_and_refreshes_same_identity() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let f = Fixture::new(family);
        fs::remove_file(f.vault_root.path().join(PROFILE_FILE)).unwrap();
        let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
        assert_eq!(
            f.store().capture(family, "absent"),
            Ok(CaptureOutcome::Saved)
        );
        assert_eq!(f.catalog().len(), 1);
        let revision = saved_revision(&f);
        assert_eq!(
            f.store().capture(family, &revision),
            Ok(CaptureOutcome::Refreshed)
        );
        assert_eq!(f.catalog().len(), 1);
        assert!(
            f.catalog().get(&f.a).unwrap().scoped_document()
                == f.native.inspect(&f.fresh).unwrap().scoped_document()
        );
        assert_ne!(saved_revision(&f), revision);
        assert_eq!(
            f.store().capture(family, &revision),
            Err(TransactionError::CatalogChanged)
        );
        assert_eq!(
            fs::read(f.native_root.path().join("credentials.json")).unwrap(),
            before
        );
        let encoded = fs::read_to_string(f.vault_root.path().join(PROFILE_FILE)).unwrap();
        assert!(encoded.starts_with("lpenc"));
        assert!(!encoded.contains("SYNTHETIC_CANARY"));
        assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    }
}
#[test]
fn io_passive_status_uses_only_saved_catalog_and_never_claims_live_identity() {
    let f = Fixture::new(OAuthFamily::Zai);
    fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
    let before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let status = f.store().status().unwrap();
    assert_eq!(status.revision, saved_revision(&f));
    assert_eq!(status.profiles.len(), 2);
    assert!(status.current.is_none());
    assert!(!status.pending);
    assert!(status.profiles.iter().any(|p| p.id == f.a.opaque_id()));
    assert!(status.profiles.iter().all(|p| p.family == "zai"));
    let dto = serde_json::to_string(&status).unwrap();
    for forbidden in [
        "enc:v1:",
        "lpenc",
        "SYNTHETIC_CANARY",
        "access_token",
        "user_info",
        "future:unrelated",
    ] {
        assert!(!dto.contains(forbidden));
    }
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        before
    );
    assert!(!f.native_root.path().join("credentials.json").exists());
}
#[test]
fn io_capture_rejects_stale_scope_pending_and_unknown_catalog_without_writes() {
    let f = Fixture::new(OAuthFamily::Zai);
    let before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let revision = saved_revision(&f);
    assert_eq!(
        f.store().capture(OAuthFamily::BigModel, &revision),
        Err(TransactionError::UnsupportedScope)
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        before
    );
    assert!(f.current() == f.fresh);
    fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
    assert_eq!(
        f.store().capture(OAuthFamily::Zai, "stale"),
        Err(TransactionError::CatalogChanged)
    );
    write_durable(
        &f.vault_root.path().join(JOURNAL_FILE),
        b"synthetic-pending",
    )
    .unwrap();
    assert!(matches!(
        f.store().status(),
        Err(TransactionError::Recovery(_))
    ));
    assert_eq!(
        f.store().capture(OAuthFamily::Zai, &revision),
        Err(TransactionError::RecoveryRequired)
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        before
    );
    fs::remove_file(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    write_durable(
        &f.vault_root.path().join(PROFILE_FILE),
        br#"{"version":999}"#,
    )
    .unwrap();
    assert!(matches!(
        f.store().capture(OAuthFamily::Zai, &saved_revision(&f)),
        Err(TransactionError::Checkpoint(_))
    ));
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        br#"{"version":999}"#
    );
    assert!(!f.native_root.path().join("credentials.json").exists());
}
#[test]
fn io_saved_id_switch_checks_revision_id_and_family_before_native_read() {
    let f = Fixture::new(OAuthFamily::Zai);
    fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
    let revision = saved_revision(&f);
    assert_eq!(
        f.store()
            .switch_saved(&f.b.opaque_id(), "stale", OAuthFamily::Zai),
        Err(TransactionError::CatalogChanged)
    );
    assert_eq!(
        f.store()
            .switch_saved("unknown", &revision, OAuthFamily::Zai),
        Err(TransactionError::MissingTarget)
    );
    assert_eq!(
        f.store()
            .switch_saved(&f.b.opaque_id(), &revision, OAuthFamily::BigModel),
        Err(TransactionError::UnsupportedScope)
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    assert!(!f.native_root.path().join("credentials.json").exists());
}
#[test]
fn io_saved_id_switch_keeps_fresh_a_b_a_for_each_family() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let f = Fixture::new(family);
        assert_eq!(
            f.store()
                .switch_saved(&f.b.opaque_id(), &saved_revision(&f), family),
            Ok(SwitchOutcome::Switched)
        );
        assert_eq!(
            f.store()
                .switch_saved(&f.a.opaque_id(), &saved_revision(&f), family),
            Ok(SwitchOutcome::Switched)
        );
        for key in f.a.credential_keys() {
            assert_eq!(f.current().get(&key), f.fresh.get(&key));
        }
    }
}

#[test]
fn io_profile_publication_rechecks_native_source_after_the_final_admission_probe() {
    for operation in ["capture", "refresh", "switch"] {
        let f = Fixture::new(OAuthFamily::Zai);
        let before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
        let revision = saved_revision(&f);
        let changed = native_document(OAuthFamily::Zai, "b", "changed-during-final-probe");
        let changed_bytes = changed.to_bytes().unwrap();
        let calls = std::cell::Cell::new(0);
        let gate = || {
            calls.set(calls.get() + 1);
            let publication = if operation == "switch" { 5 } else { 4 };
            if calls.get() == publication {
                write_durable(
                    &f.native_root.path().join("credentials.json"),
                    &changed_bytes,
                )
                .unwrap();
            }
            Ok(())
        };
        let store = AccountStore::new_guarded(
            f.native_root.path(),
            f.vault_root.path(),
            &f.vault,
            &f.native,
            ADMITTED,
            &gate,
        )
        .unwrap();
        let result = match operation {
            "capture" => store.capture(OAuthFamily::Zai, &revision).map(|_| ()),
            "refresh" => store.switch(&f.a).map(|_| ()),
            _ => store.switch(&f.b).map(|_| ()),
        };
        assert_eq!(result, Err(TransactionError::SourceChanged), "{operation}");
        assert_eq!(
            fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
            before
        );
        assert!(f.current() == changed);
        assert_eq!(
            f.vault_root.path().join(JOURNAL_FILE).exists(),
            operation == "switch"
        );
    }
}

fn leave_captured_journal(f: &Fixture) -> JournalEvidence {
    assert!(f
        .store()
        .switch_with_hook(&f.b, &mut |point| {
            if point == WritePoint::Captured {
                Err(TransactionError::Storage)
            } else {
                Ok(())
            }
        })
        .is_err());
    JournalEvidence::open_journal(
        &fs::read_to_string(f.vault_root.path().join(JOURNAL_FILE)).unwrap(),
        &f.vault,
    )
    .unwrap()
}
#[test]
fn vault_only_archive_retains_evidence_without_any_native_file_or_cipher() {
    let f = Fixture::new(OAuthFamily::Zai);
    let evidence = leave_captured_journal(&f);
    let profile = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let status = local.status().unwrap();
    assert!(status.pending);
    assert_eq!(
        local.archive_pending(&status.revision),
        Ok(ArchiveOutcome::Archived)
    );
    let status = local.status().unwrap();
    assert!(!status.pending && status.native_unconfirmed);
    assert_eq!(status.records.len(), 1);
    assert_eq!(status.records[0].id, evidence.id());
    let ledger = RecoveryLedger::open(
        &fs::read_to_string(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
        &f.vault,
    )
    .unwrap();
    assert_eq!(
        ledger.archived().next().unwrap().evidence().raw_payload(),
        evidence.raw_payload()
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        profile
    );
    assert!(!f.native_root.path().join("credentials.json").exists());
    assert_eq!(
        local.archive_pending(&status.revision),
        Ok(ArchiveOutcome::NothingPending)
    );
    assert_eq!(
        local.delete_confirmed(evidence.id(), &status.revision),
        Err(TransactionError::Recovery(RecoveryError::Unconfirmed))
    );
}
#[test]
fn vault_only_archive_cas_keeps_journal_when_published_record_or_source_is_replaced() {
    for replace in [Role::Recovery, Role::Journal] {
        let f = Fixture::new(OAuthFamily::Zai);
        leave_captured_journal(&f);
        let native = f.current();
        let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
        let status = local.status().unwrap();
        let result = local.archive_with_hook(&status.revision, &mut |point| {
            if point == ArchivePoint::RecoveryPublished {
                let path = f.vault_root.path().join(replace.name());
                let bytes = fs::read(&path).unwrap();
                write_durable(&path, &bytes).unwrap();
            }
            Ok(())
        });
        assert_eq!(result, Err(TransactionError::SourceChanged));
        assert!(f.vault_root.path().join(JOURNAL_FILE).exists());
        assert!(f.current() == native);
    }
}
#[test]
fn vault_only_archive_rejects_stale_revision_and_unknown_existing_record_without_loss() {
    let f = Fixture::new(OAuthFamily::Zai);
    leave_captured_journal(&f);
    let journal = fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    assert_eq!(
        local.archive_pending("stale"),
        Err(TransactionError::RecoveryChanged)
    );
    write_durable(
        &f.vault_root.path().join(RECOVERY_FILE),
        b"unsupported protected format",
    )
    .unwrap();
    assert!(local.status().is_err());
    assert!(local.archive_pending("stale").is_err());
    assert_eq!(
        fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap(),
        journal
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
        b"unsupported protected format"
    );
}

#[test]
fn io_unconfirmed_archive_blocks_native_operations_before_native_read() {
    let f = Fixture::new(OAuthFamily::Zai);
    leave_captured_journal(&f);
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    local
        .archive_pending(&local.status().unwrap().revision)
        .unwrap();
    fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
    assert_eq!(
        f.store().capture(OAuthFamily::Zai, &saved_revision(&f)),
        Err(TransactionError::NativeUnconfirmed)
    );
    assert_eq!(
        f.store()
            .switch_saved(&f.b.opaque_id(), &saved_revision(&f), OAuthFamily::Zai),
        Err(TransactionError::NativeUnconfirmed)
    );
    assert_eq!(
        f.store().switch(&f.a),
        Err(TransactionError::NativeUnconfirmed)
    );
    assert!(!f.native_root.path().join("credentials.json").exists());
}
#[test]
fn io_switch_reserves_archive_capacity_before_any_publication() {
    use super::super::recovery::RecoveryConfirmation;
    let f = Fixture::new(OAuthFamily::Zai);
    let mut ledger = RecoveryLedger::default();
    for _ in 0..2 {
        let evidence = leave_captured_journal(&f);
        let proof = RecoveryConfirmation::from_image(&evidence, &f.current(), &f.native).unwrap();
        let id = ledger.archive(evidence).unwrap();
        ledger.confirm(&id, proof).unwrap();
        fs::remove_file(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    }
    write_durable(
        &f.vault_root.path().join(RECOVERY_FILE),
        ledger.seal(&f.vault).unwrap().as_bytes(),
    )
    .unwrap();
    let profile = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let native = f.current();
    assert_eq!(
        f.store()
            .switch_saved(&f.b.opaque_id(), &saved_revision(&f), OAuthFamily::Zai),
        Err(TransactionError::Recovery(RecoveryError::ArchiveFull))
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    assert_eq!(
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
        profile
    );
    assert!(f.current() == native);
}

#[test]
fn io_archive_confirmation_preserves_native_and_allows_only_whole_known_images() {
    for mix in [false, true] {
        let f = Fixture::new(OAuthFamily::Zai);
        let evidence = leave_captured_journal(&f);
        let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
        local
            .archive_pending(&local.status().unwrap().revision)
            .unwrap();
        if mix {
            let target = f.catalog().get(&f.b).unwrap().scoped_document();
            let changed = replace(
                &target,
                "oauth:zai:access_token",
                f.fresh.get("oauth:zai:access_token").unwrap().to_owned(),
            );
            write_durable(
                &f.native_root.path().join("credentials.json"),
                &changed.to_bytes().unwrap(),
            )
            .unwrap();
        }
        let native = fs::read(f.native_root.path().join("credentials.json")).unwrap();
        let status = local.status().unwrap();
        let result = f.store().confirm_archived(evidence.id(), &status.revision);
        if mix {
            assert_eq!(
                result,
                Err(TransactionError::Recovery(
                    RecoveryError::ConfirmationMismatch
                ))
            );
            assert!(local.status().unwrap().native_unconfirmed);
        } else {
            assert_eq!(result, Ok(()));
            let confirmed = local.status().unwrap();
            assert!(!confirmed.native_unconfirmed);
            assert_eq!(
                local.delete_confirmed(evidence.id(), &status.revision),
                Err(TransactionError::RecoveryChanged)
            );
            assert_eq!(
                local.delete_confirmed(evidence.id(), &confirmed.revision),
                Ok(())
            );
            assert!(!f.vault_root.path().join(RECOVERY_FILE).exists());
        }
        assert_eq!(
            fs::read(f.native_root.path().join("credentials.json")).unwrap(),
            native
        );
    }
}
#[test]
fn io_explicit_recapture_persists_new_login_before_confirmation_without_native_writes() {
    let f = Fixture::new(OAuthFamily::Zai);
    let evidence = leave_captured_journal(&f);
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    local
        .archive_pending(&local.status().unwrap().revision)
        .unwrap();
    let current = native_document(OAuthFamily::Zai, "c", "official-relogin");
    let identity = f.native.inspect(&current).unwrap().identity().clone();
    write_durable(
        &f.native_root.path().join("credentials.json"),
        &current.to_bytes().unwrap(),
    )
    .unwrap();
    let revision = local.status().unwrap().revision;
    assert_eq!(
        f.store().capture_and_confirm(
            evidence.id(),
            &revision,
            &saved_revision(&f),
            OAuthFamily::Zai
        ),
        Ok(CaptureOutcome::Saved)
    );
    assert!(f.current() == current);
    assert!(
        f.catalog().get(&identity).unwrap().scoped_document()
            == f.native.inspect(&current).unwrap().scoped_document()
    );
    assert!(!local.status().unwrap().native_unconfirmed);
    let ledger = RecoveryLedger::open(
        &fs::read_to_string(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
        &f.vault,
    )
    .unwrap();
    assert_eq!(
        ledger.archived().next().unwrap().evidence().raw_payload(),
        evidence.raw_payload()
    );
}

#[test]
fn io_old_journal_after_newer_completion_never_replays_native_or_overwrites_latest() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let f = Fixture::new(family);
        let old = leave_captured_journal(&f);
        let encoded = fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
        archive_and_confirm(&f, &f.store());
        f.store().switch(&f.b).unwrap();
        let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
        let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
        let latest = local
            .status()
            .unwrap()
            .records
            .into_iter()
            .find(|record| record.latest_completed)
            .unwrap()
            .id;
        write_durable(&f.vault_root.path().join(JOURNAL_FILE), &encoded).unwrap();
        local
            .archive_pending(&local.status().unwrap().revision)
            .unwrap();
        let status = local.status().unwrap();
        assert!(status.native_unconfirmed);
        assert_eq!(status.records.len(), 2);
        assert!(status
            .records
            .iter()
            .any(|record| record.latest_completed && record.id == latest));
        f.store()
            .confirm_archived(old.id(), &status.revision)
            .unwrap();
        assert_eq!(
            fs::read(f.native_root.path().join("credentials.json")).unwrap(),
            before
        );
        assert!(f.native.inspect(&f.current()).unwrap().identity() == &f.b);
        assert_eq!(local.status().unwrap().records.len(), 2);
    }
}

#[test]
fn io_cleanup_is_selected_revision_bound_and_cancel_is_read_only() {
    let f = Fixture::new(OAuthFamily::Zai);
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let first = leave_captured_journal(&f);
    archive_and_confirm(&f, &f.store());
    let second = leave_captured_journal(&f);
    archive_and_confirm(&f, &f.store());
    let before = fs::read(f.vault_root.path().join(RECOVERY_FILE)).unwrap();
    let preview = local.status().unwrap();
    // A dismissed future UI confirmation dispatches no deletion. The only
    // preview operation is read-only, and stale/unknown selections cannot clear.
    assert_eq!(local.status().unwrap().revision, preview.revision);
    assert_eq!(
        fs::read(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
        before
    );
    assert_eq!(
        local.delete_confirmed("unknown-record", &preview.revision),
        Err(TransactionError::Recovery(RecoveryError::NotFound))
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
        before
    );
    local
        .delete_confirmed(first.id(), &preview.revision)
        .unwrap();
    let after = local.status().unwrap();
    assert_eq!(after.records.len(), 1);
    assert_eq!(after.records[0].id, second.id());
    assert_eq!(
        local.delete_confirmed(second.id(), &preview.revision),
        Err(TransactionError::RecoveryChanged)
    );
    assert_eq!(f.store().switch(&f.b), Ok(SwitchOutcome::Switched));
    let status = local.status().unwrap();
    assert_eq!(status.records.len(), 2);
    assert!(status
        .records
        .iter()
        .any(|record| record.id == second.id() && !record.latest_completed));
    assert!(status.records.iter().any(|record| record.latest_completed));
}

#[test]
fn io_recapture_final_native_cas_leaves_evidence_unconfirmed_and_saved_profile_intact() {
    let f = Fixture::new(OAuthFamily::Zai);
    let evidence = leave_captured_journal(&f);
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    local
        .archive_pending(&local.status().unwrap().revision)
        .unwrap();
    let current = native_document(OAuthFamily::Zai, "c", "official-relogin");
    let identity = f.native.inspect(&current).unwrap().identity().clone();
    write_durable(
        &f.native_root.path().join("credentials.json"),
        &current.to_bytes().unwrap(),
    )
    .unwrap();
    let original_profile = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let changed = replace(&current, "future:concurrent-writer", "changed".into())
        .to_bytes()
        .unwrap();
    let modified = std::cell::Cell::new(false);
    let gate = || {
        if !modified.get()
            && fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap() != original_profile
        {
            write_durable(&f.native_root.path().join("credentials.json"), &changed).unwrap();
            modified.set(true);
        }
        Ok(())
    };
    let store = AccountStore::new_guarded(
        f.native_root.path(),
        f.vault_root.path(),
        &f.vault,
        &f.native,
        ADMITTED,
        &gate,
    )
    .unwrap();
    assert_eq!(
        store.capture_and_confirm(
            evidence.id(),
            &local.status().unwrap().revision,
            &saved_revision(&f),
            OAuthFamily::Zai
        ),
        Err(TransactionError::SourceChanged)
    );
    assert!(modified.get());
    assert!(f.catalog().get(&identity).is_some());
    assert!(local.status().unwrap().native_unconfirmed);
    assert_eq!(
        fs::read(f.native_root.path().join("credentials.json")).unwrap(),
        changed
    );
}

#[test]
#[ignore = "invoked only by the synthetic archive subprocess matrix"]
fn synthetic_archive_crash_child() {
    use std::io::Read;
    assert_eq!(
        std::env::var("LOONGPORT_ZCODE_SYNTHETIC_CHILD").as_deref(),
        Ok("1")
    );
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    std::io::stdin()
        .take(16384)
        .read_to_end(&mut bytes)
        .unwrap();
    let input: ChildInput = serde_json::from_slice(&bytes).unwrap();
    let vault = VaultContext::from_key(input.metadata, zeroize::Zeroizing::new(input.key)).unwrap();
    let local = VaultAccountStore::new(&input.vault_root, &vault).unwrap();
    local
        .archive_with_hook(&local.status().unwrap().revision, &mut |point| {
            if (point == ArchivePoint::RecoveryPublished && input.checkpoint == 0)
                || (point == ArchivePoint::JournalRemoved && input.checkpoint == 1)
            {
                std::process::exit(74);
            }
            Ok(())
        })
        .unwrap();
    panic!("requested archive crash point was not reached");
}

#[test]
fn io_archive_real_process_crashes_preserve_evidence_and_repeat_without_native_reads() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for checkpoint in 0..2 {
        let f = Fixture::new(OAuthFamily::Zai);
        let evidence = leave_captured_journal(&f);
        fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
        let input = ChildInput {
            native_root: f.native_root.path().into(),
            vault_root: f.vault_root.path().into(),
            metadata: f.vault.metadata().clone(),
            key: f.vault.export_key().to_vec(),
            checkpoint,
        };
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "zcode_accounts::transaction::tests::synthetic_archive_crash_child",
                "--nocapture",
            ])
            .env("LOONGPORT_ZCODE_SYNTHETIC_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&input).unwrap());
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(74),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let vault =
            VaultContext::from_key(f.vault.metadata().clone(), f.vault.export_key()).unwrap();
        let local = VaultAccountStore::new(f.vault_root.path(), &vault).unwrap();
        assert_eq!(local.status().unwrap().pending, checkpoint == 0);
        assert!(local.status().unwrap().native_unconfirmed);
        assert_eq!(
            local.archive_pending(&local.status().unwrap().revision),
            Ok(if checkpoint == 0 {
                ArchiveOutcome::Archived
            } else {
                ArchiveOutcome::NothingPending
            })
        );
        let ledger = RecoveryLedger::open(
            &fs::read_to_string(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
            &vault,
        )
        .unwrap();
        assert_eq!(ledger.archived().count(), 1);
        assert_eq!(
            ledger.archived().next().unwrap().evidence().raw_payload(),
            evidence.raw_payload()
        );
        assert!(!f.native_root.path().join("credentials.json").exists());
    }
}

#[test]
fn io_full_archive_can_clear_only_confirmed_old_record_while_preserving_pending_journal() {
    let f = Fixture::new(OAuthFamily::Zai);
    let mut journals = Vec::new();
    for _ in 0..3 {
        let evidence = leave_captured_journal(&f);
        let encoded = fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
        journals.push((evidence, encoded));
        fs::remove_file(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    }
    let mut ledger = RecoveryLedger::default();
    for (evidence, _) in &journals[..2] {
        let proof = RecoveryConfirmation::from_image(evidence, &f.current(), &f.native).unwrap();
        let id = ledger.archive(evidence.clone()).unwrap();
        ledger.confirm(&id, proof).unwrap();
    }
    write_durable(
        &f.vault_root.path().join(RECOVERY_FILE),
        ledger.seal(&f.vault).unwrap().as_bytes(),
    )
    .unwrap();
    write_durable(&f.vault_root.path().join(JOURNAL_FILE), &journals[2].1).unwrap();
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let status = local.status().unwrap();
    assert_eq!(
        local.archive_pending(&status.revision),
        Err(TransactionError::Recovery(RecoveryError::ArchiveFull))
    );
    local
        .delete_confirmed(journals[0].0.id(), &status.revision)
        .unwrap();
    assert_eq!(
        fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap(),
        journals[2].1
    );
    let freed = local.status().unwrap();
    assert!(freed.pending && freed.native_unconfirmed);
    assert_eq!(freed.records.len(), 1);
    assert_eq!(freed.records[0].id, journals[1].0.id());
    local.archive_pending(&freed.revision).unwrap();
    let archived = local.status().unwrap();
    assert_eq!(archived.records.len(), 2);
    assert_eq!(
        local.delete_confirmed(journals[2].0.id(), &archived.revision),
        Err(TransactionError::Recovery(RecoveryError::Unconfirmed))
    );
    assert!(f.current() == f.fresh);
}

#[test]
fn io_confirmed_cleanup_checks_original_journal_identity_and_reappeared_evidence() {
    let f = Fixture::new(OAuthFamily::Zai);
    let evidence = leave_captured_journal(&f);
    let original_journal = fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    archive_and_confirm(&f, &f.store());
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    write_durable(&f.vault_root.path().join(JOURNAL_FILE), &original_journal).unwrap();
    let status = local.status().unwrap();
    assert_eq!(status.records[0].disposition, "native-unconfirmed");
    assert_eq!(
        local.delete_confirmed(evidence.id(), &status.revision),
        Err(TransactionError::Recovery(RecoveryError::Unconfirmed))
    );
    archive_and_confirm(&f, &f.store());
    let other = leave_captured_journal(&f);
    assert_ne!(other.id(), evidence.id());
    let journal = fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    let recovery = fs::read(f.vault_root.path().join(RECOVERY_FILE)).unwrap();
    let status = local.status().unwrap();
    let pinned = fs::File::open(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    assert_eq!(
        local.delete_with_hook(evidence.id(), &status.revision, &mut || {
            write_durable(&f.vault_root.path().join(JOURNAL_FILE), &journal).unwrap();
            Ok(())
        }),
        Err(TransactionError::SourceChanged)
    );
    drop(pinned);
    assert_eq!(
        fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap(),
        journal
    );
    assert_eq!(
        fs::read(f.vault_root.path().join(RECOVERY_FILE)).unwrap(),
        recovery
    );
}

#[test]
fn io_full_unconfirmed_archive_allows_explicit_recapture_then_selected_cleanup_with_pending_journal(
) {
    let f = Fixture::new(OAuthFamily::Zai);
    let mut journals = Vec::new();
    for _ in 0..3 {
        let evidence = leave_captured_journal(&f);
        journals.push((
            evidence,
            fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap(),
        ));
        fs::remove_file(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
    }
    let mut ledger = RecoveryLedger::default();
    for (evidence, _) in &journals[..2] {
        ledger.archive(evidence.clone()).unwrap();
    }
    write_durable(
        &f.vault_root.path().join(RECOVERY_FILE),
        ledger.seal(&f.vault).unwrap().as_bytes(),
    )
    .unwrap();
    write_durable(&f.vault_root.path().join(JOURNAL_FILE), &journals[2].1).unwrap();
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let status = local.status().unwrap();
    assert_eq!(
        local.archive_pending(&status.revision),
        Err(TransactionError::Recovery(RecoveryError::ArchiveFull))
    );
    assert_eq!(
        local.delete_confirmed(journals[0].0.id(), &status.revision),
        Err(TransactionError::Recovery(RecoveryError::Unconfirmed))
    );
    let current = native_document(OAuthFamily::Zai, "c", "official-relogin");
    write_durable(
        &f.native_root.path().join("credentials.json"),
        &current.to_bytes().unwrap(),
    )
    .unwrap();
    f.store()
        .capture_and_confirm(
            journals[0].0.id(),
            &status.revision,
            &saved_revision(&f),
            OAuthFamily::Zai,
        )
        .unwrap();
    let captured = local.status().unwrap();
    assert!(captured.pending && captured.native_unconfirmed);
    assert_eq!(
        fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap(),
        journals[2].1
    );
    assert_eq!(
        f.store().switch(&f.b),
        Err(TransactionError::RecoveryRequired)
    );
    local
        .delete_confirmed(journals[0].0.id(), &captured.revision)
        .unwrap();
    local
        .archive_pending(&local.status().unwrap().revision)
        .unwrap();
    assert!(!local.status().unwrap().pending);
    assert!(local.status().unwrap().native_unconfirmed);
    assert!(f.current() == current);
}

#[test]
fn io_ordinary_publications_recheck_journal_and_recovery_after_final_gate() {
    let mut failures = Vec::new();
    for operation in ["capture", "refresh", "switch"] {
        for role in [Role::Journal, Role::Recovery] {
            let f = Fixture::new(OAuthFamily::Zai);
            let evidence = leave_captured_journal(&f);
            let old_journal = fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
            fs::remove_file(f.vault_root.path().join(JOURNAL_FILE)).unwrap();
            let mut ledger = RecoveryLedger::default();
            ledger.archive(evidence).unwrap();
            let restored_recovery = ledger.seal(&f.vault).unwrap().into_bytes();
            if operation == "capture" {
                let current = native_document(OAuthFamily::Zai, "c", "explicit-capture");
                write_durable(
                    &f.native_root.path().join("credentials.json"),
                    &current.to_bytes().unwrap(),
                )
                .unwrap();
            }
            let native_before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
            let profile_before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
            let revision = saved_revision(&f);
            let calls = std::cell::Cell::new(0);
            let gate = || {
                calls.set(calls.get() + 1);
                if calls.get() == if operation == "switch" { 7 } else { 4 } {
                    let data = match role {
                        Role::Recovery => restored_recovery.clone(),
                        Role::Journal if operation == "switch" => {
                            fs::read(f.vault_root.path().join(JOURNAL_FILE)).unwrap()
                        }
                        Role::Journal => old_journal.clone(),
                        _ => unreachable!(),
                    };
                    write_durable(&f.vault_root.path().join(role.name()), &data).unwrap();
                }
                Ok(())
            };
            let store = AccountStore::new_guarded(
                f.native_root.path(),
                f.vault_root.path(),
                &f.vault,
                &f.native,
                ADMITTED,
                &gate,
            )
            .unwrap();
            let result = match operation {
                "capture" => store.capture(OAuthFamily::Zai, &revision).map(|_| ()),
                "refresh" => store.switch(&f.a).map(|_| ()),
                "switch" => store.switch(&f.b).map(|_| ()),
                _ => unreachable!(),
            };
            let native_changed =
                fs::read(f.native_root.path().join("credentials.json")).unwrap() != native_before;
            let profile_changed =
                fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap() != profile_before;
            println!("{operation}/{}: result={result:?}, native_changed={native_changed}, profile_changed={profile_changed}", role.name());
            if result != Err(TransactionError::SourceChanged)
                || native_changed
                || (operation != "switch" && profile_changed)
            {
                failures.push(format!("{operation}/{}: {result:?}, native_changed={native_changed}, profile_changed={profile_changed}", role.name()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn io_official_native_directory_may_be_readable_but_never_writable_by_others() {
    use std::os::unix::fs::PermissionsExt;
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let f = Fixture::new(family);
        // The official private-file writer uses recursive mkdir's normal mode,
        // while credentials and temporary files are always created as 0600.
        fs::set_permissions(f.native_root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(f.store().switch(&f.b), Ok(SwitchOutcome::Switched));
        assert_eq!(
            fs::metadata(f.native_root.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(f.native_root.path().join("credentials.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    for mode in [0o770, 0o777, 0o722, 0o702] {
        let f = Fixture::new(OAuthFamily::Zai);
        let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
        fs::set_permissions(f.native_root.path(), fs::Permissions::from_mode(mode)).unwrap();
        assert!(matches!(
            AccountStore::new(
                f.native_root.path(),
                f.vault_root.path(),
                &f.vault,
                &f.native,
                ADMITTED
            ),
            Err(TransactionError::UnsafePath)
        ));
        assert_eq!(
            fs::read(f.native_root.path().join("credentials.json")).unwrap(),
            before
        );
        assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    }
}
#[test]
fn io_vault_root_and_credential_file_remain_private_with_readable_native_directory() {
    use std::os::unix::fs::PermissionsExt;
    for widen_vault in [false, true] {
        let f = Fixture::new(OAuthFamily::Zai);
        fs::set_permissions(f.native_root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
        let profile = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
        if widen_vault {
            fs::set_permissions(f.vault_root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        } else {
            fs::set_permissions(
                f.native_root.path().join("credentials.json"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }
        let result = AccountStore::new(
            f.native_root.path(),
            f.vault_root.path(),
            &f.vault,
            &f.native,
            ADMITTED,
        )
        .and_then(|store| store.switch(&f.b));
        assert_eq!(result, Err(TransactionError::UnsafePath));
        assert_eq!(
            fs::read(f.native_root.path().join("credentials.json")).unwrap(),
            before
        );
        assert_eq!(
            fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
            profile
        );
        assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
    }
}

#[test]
fn io_native_root_rejects_symlink_alias_and_foreign_ownership_metadata() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new(OAuthFamily::Zai);
    let aliases = tempfile::tempdir().unwrap();
    let alias = aliases.path().join("native-alias");
    symlink(f.native_root.path(), &alias).unwrap();
    assert_eq!(
        native_root_identity(&alias),
        Err(TransactionError::UnsafePath)
    );
    let metadata = fs::symlink_metadata(f.native_root.path()).unwrap();
    let current_uid = unsafe { libc::geteuid() };
    assert!(directory_metadata_is_safe(&metadata, current_uid, 0o022));
    // Exercise the owner predicate with real metadata without chown privileges or
    // touching another account's files. Production always uses the actual euid.
    assert!(!directory_metadata_is_safe(
        &metadata,
        current_uid.wrapping_add(1),
        0o022
    ));
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
}

#[test]
fn io_operation_recovers_authenticated_commit_without_native_read_or_replay() {
    use super::super::operation_log::{Phase, Record};
    let f = Fixture::new(OAuthFamily::Zai);
    let local = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    local
        .record_operation(Record {
            request_id: id.clone(),
            context_id: TEST_CONTEXT.into(),
            native_root: root_identity(f.native_root.path()).unwrap(),
            target: f.b.opaque_id(),
            phase: Phase::TransactionUncertain,
            refreshed: false,
            restart_requested: true,
        })
        .unwrap();
    let store = f.store();
    let status = store.status().unwrap();
    assert_eq!(
        store.switch_saved_operation(&f.b.opaque_id(), &status.revision, OAuthFamily::Zai, &id),
        Ok(SwitchOutcome::Switched)
    );
    fs::remove_file(f.native_root.path().join("credentials.json")).unwrap();
    assert_eq!(
        local.recover_operation(&id).unwrap().unwrap().phase,
        Phase::Committed
    );
    assert!(!f.native_root.path().join("credentials.json").exists());
}
#[test]
fn io_operation_missing_saved_source_is_proven_before_first_publish() {
    let f = Fixture::new(OAuthFamily::Zai);
    let unknown = native_document(OAuthFamily::Zai, "not-saved", "fresh");
    write_durable(
        &f.native_root.path().join("credentials.json"),
        &unknown.to_bytes().unwrap(),
    )
    .unwrap();
    let store = f.store();
    let status = store.status().unwrap();
    let mut started = false;
    assert_eq!(
        store.switch_saved_operation_started(
            &f.b.opaque_id(),
            &status.revision,
            OAuthFamily::Zai,
            &uuid::Uuid::new_v4().to_string(),
            &mut || {
                started = true;
            }
        ),
        Err(TransactionError::MissingSavedSource)
    );
    assert!(!started);
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
}

#[test]
fn login_save_receipt_is_atomic_idempotent_and_survives_lost_reply() {
    let fixture = Fixture::new(OAuthFamily::Zai);
    let local = VaultAccountStore::new(fixture.vault_root.path(), &fixture.vault).unwrap();
    let status = local.catalog_status(&fixture.native).unwrap();
    let snapshot = fixture
        .native
        .inspect(&native_document(OAuthFamily::Zai, "new-login", "synthetic"))
        .unwrap();
    let id = snapshot.identity().opaque_id();
    let request = uuid::Uuid::new_v4().to_string();
    let native_before = fs::read(fixture.native_root.path().join("credentials.json")).unwrap();
    assert_eq!(
        local
            .save_login_profile(
                &fixture.native,
                &status.revision,
                &request,
                snapshot.clone(),
                None,
                false
            )
            .unwrap(),
        CaptureCommitOutcome::Saved
    );
    let bytes = fs::read(fixture.vault_root.path().join(PROFILE_FILE)).unwrap();
    let receipt = local
        .login_receipt(&fixture.native, &request)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.account_id, id);
    assert_eq!(receipt.outcome, CaptureCommitOutcome::Saved);
    assert_eq!(
        local
            .save_login_profile(
                &fixture.native,
                &status.revision,
                &request,
                snapshot,
                None,
                false
            )
            .unwrap(),
        CaptureCommitOutcome::Saved
    );
    assert_eq!(
        bytes,
        fs::read(fixture.vault_root.path().join(PROFILE_FILE)).unwrap()
    );
    assert_eq!(
        native_before,
        fs::read(fixture.native_root.path().join("credentials.json")).unwrap()
    );
    let other = fixture
        .native
        .inspect(&native_document(
            OAuthFamily::Zai,
            "other-login",
            "synthetic",
        ))
        .unwrap();
    assert!(local
        .save_login_profile(
            &fixture.native,
            &status.revision,
            &request,
            other,
            None,
            false
        )
        .is_err());
}

#[test]
fn saved_coding_save_binds_original_revision_target_and_source_without_other_mutation() {
    let fixture = Fixture::new(OAuthFamily::Zai);
    let local = VaultAccountStore::new(fixture.vault_root.path(), &fixture.vault).unwrap();
    let status = local.catalog_status(&fixture.native).unwrap();
    let catalog = local
        .import_catalog(&fixture.native, &status.revision)
        .unwrap();
    let original = catalog.get(&fixture.a).unwrap().clone();
    let target = super::super::oauth::SavedCodingTarget {
        snapshot: original.clone(),
        revision: status.revision.clone(),
        details: catalog.details(&original, &fixture.native),
    };
    let candidate = super::super::oauth_account::complete_coding_snapshot(
        &fixture.native,
        &original,
        Some(&super::super::official::CodingKey::new("replacement-coding").unwrap()),
    )
    .unwrap();
    let native_before = fs::read(fixture.native_root.path().join("credentials.json")).unwrap();
    let other_before = catalog
        .get(&fixture.b)
        .unwrap()
        .scoped_document()
        .to_bytes()
        .unwrap();
    let request_id = uuid::Uuid::new_v4().to_string();
    let request = |revision| LoginProfileSave {
        revision,
        request_id: &request_id,
        snapshot: candidate.clone(),
        evidence: None,
        update_duplicate: true,
        completion: Some(&target),
    };
    assert_eq!(
        local.save_login_profile_for(&fixture.native, request("stale-revision")),
        Err(TransactionError::CatalogChanged)
    );
    let mut wrong = request(&status.revision);
    wrong.snapshot = catalog.get(&fixture.b).unwrap().clone();
    assert_eq!(
        local.save_login_profile_for(&fixture.native, wrong),
        Err(TransactionError::SourceChanged)
    );
    assert_eq!(
        local
            .save_login_profile_for(&fixture.native, request(&status.revision))
            .unwrap(),
        CaptureCommitOutcome::Refreshed
    );
    assert_eq!(
        local
            .save_login_profile_for(&fixture.native, request(&status.revision))
            .unwrap(),
        CaptureCommitOutcome::Refreshed
    );
    let updated = fixture.catalog();
    let saved = updated.get(&fixture.a).unwrap();
    assert!(super::super::oauth_account::coding_only_change(
        &original, saved
    ));
    assert_eq!(
        updated.details(saved, &fixture.native).identity_source,
        super::super::checkpoint::IdentitySource::NativeCapture
    );
    assert_eq!(
        updated
            .get(&fixture.b)
            .unwrap()
            .scoped_document()
            .to_bytes()
            .unwrap(),
        other_before
    );
    assert_eq!(
        fs::read(fixture.native_root.path().join("credentials.json")).unwrap(),
        native_before
    );
}

#[test]
fn saved_coding_decline_records_kept_receipt_and_preserves_start_metadata() {
    let fixture = Fixture::new(OAuthFamily::Zai);
    let local = VaultAccountStore::new(fixture.vault_root.path(), &fixture.vault).unwrap();
    let status = local.catalog_status(&fixture.native).unwrap();
    let catalog = local
        .import_catalog(&fixture.native, &status.revision)
        .unwrap();
    let original = catalog.get(&fixture.a).unwrap().clone();
    let target = super::super::oauth::SavedCodingTarget {
        snapshot: original.clone(),
        revision: status.revision.clone(),
        details: catalog.details(&original, &fixture.native),
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        local
            .save_login_profile_for(
                &fixture.native,
                LoginProfileSave {
                    revision: &status.revision,
                    request_id: &request_id,
                    snapshot: original.clone(),
                    evidence: None,
                    update_duplicate: true,
                    completion: Some(&target)
                }
            )
            .unwrap(),
        CaptureCommitOutcome::Kept
    );
    let updated = fixture.catalog();
    assert!(updated.source_verified(&fixture.a));
    assert_eq!(
        updated
            .get(&fixture.a)
            .unwrap()
            .scoped_document()
            .to_bytes()
            .unwrap(),
        original.scoped_document().to_bytes().unwrap()
    );
    assert_eq!(
        local
            .login_receipt(&fixture.native, &request_id)
            .unwrap()
            .unwrap()
            .outcome,
        CaptureCommitOutcome::Kept
    );
}

#[test]
fn saved_coding_eligibility_uses_only_selected_credentials_and_keeps_internal_marker_private() {
    let fixture = Fixture::new(OAuthFamily::Zai);
    let original = fixture.native.inspect(&fixture.fresh).unwrap();
    let keys = original.identity().credential_keys();
    for has_business in [true, false] {
        let mut fields: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&original.scoped_document().to_bytes().unwrap()).unwrap();
        fields.remove(&keys[5]);
        if !has_business {
            fields.insert(
                keys[1].clone(),
                fixture.native.encrypt("invalid\nheader").unwrap(),
            );
        }
        let snapshot = fixture
            .native
            .inspect(&CredentialDocument::parse(&serde_json::to_vec(&fields).unwrap()).unwrap())
            .unwrap();
        let mut catalog = ProfileCatalog::default();
        catalog.upsert_unverified(snapshot.clone());
        let row = saved_account(&fixture.native, &catalog, &snapshot).unwrap();
        assert!(row.needs_key);
        assert_eq!(row.can_complete_coding, has_business);
        assert_eq!(row.complete_coding_blocked_reason.is_some(), !has_business);
        let dto = serde_json::to_string(&row).unwrap();
        assert!(!dto.contains("preservedNativeStart"));
        assert!(!dto.contains("saved-business"));
        assert_eq!(
            snapshot.scoped_document().get(&keys[4]),
            original.scoped_document().get(&keys[4])
        );
    }
}
#[test]
fn login_duplicate_default_keep_also_has_a_durable_receipt_without_changing_old_session() {
    let fixture = Fixture::new(OAuthFamily::Zai);
    let local = VaultAccountStore::new(fixture.vault_root.path(), &fixture.vault).unwrap();
    let before = local.catalog_status(&fixture.native).unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let candidate = fixture.native.inspect(&fixture.fresh).unwrap();
    let original = fixture
        .catalog()
        .get(&fixture.a)
        .unwrap()
        .scoped_document()
        .to_bytes()
        .unwrap();
    assert_eq!(
        local
            .save_login_profile(
                &fixture.native,
                &before.revision,
                &request,
                candidate,
                None,
                false
            )
            .unwrap(),
        CaptureCommitOutcome::Kept
    );
    assert_eq!(
        fixture
            .catalog()
            .get(&fixture.a)
            .unwrap()
            .scoped_document()
            .to_bytes()
            .unwrap(),
        original
    );
    assert_eq!(
        local
            .login_receipt(&fixture.native, &request)
            .unwrap()
            .unwrap()
            .outcome,
        CaptureCommitOutcome::Kept
    );
}

async fn checked_report(
    native: &NativeCipher,
    snapshot: &AccountSnapshot,
    available: bool,
) -> super::super::session_checks::SessionCheckReport {
    use super::super::official::*;
    struct Synthetic(bool);
    impl OfficialTransport for Synthetic {
        fn send(&self, request: OfficialRequest) -> TransportFuture<'_> {
            let (status, data) = if !self.0 {
                (503, serde_json::json!({}))
            } else if request.url.contains("getCustomerInfo") {
                (401, serde_json::json!({}))
            } else if request.url.contains("billing/balance") {
                (
                    200,
                    serde_json::json!({"plans":[{"status":"active","plan_id":"zcode-v3-start-plan"}],"balances":[]}),
                )
            } else if request.url.contains("subscription/list") {
                (
                    200,
                    serde_json::json!([{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]),
                )
            } else {
                (200, serde_json::json!({"limits":[]}))
            };
            Box::pin(async move {
                Ok(OfficialResponse {
                    status,
                    body: zeroize::Zeroizing::new(
                        serde_json::to_vec(&serde_json::json!({"code":0,"data":data})).unwrap(),
                    ),
                })
            })
        }
    }
    super::super::session_checks::check_session(
        &OfficialClient::new(Synthetic(available)),
        native,
        snapshot,
        Some("3.14.4"),
        100,
        &|| Ok(()),
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn checked_complete_import_switches_selected_capability_without_business_identity_gate() {
    use super::super::checkpoint::{ConnectionKind, IdentitySource};
    let fixture = Fixture::new(OAuthFamily::Zai);
    let snapshot = fixture
        .native
        .inspect(&native_document(OAuthFamily::Zai, "b", "external"))
        .unwrap();
    let report = checked_report(&fixture.native, &snapshot, true).await;
    assert_eq!(
        report.display().business.check.state,
        super::super::session_checks::CheckState::Unavailable
    );
    let local = VaultAccountStore::new(fixture.vault_root.path(), &fixture.vault).unwrap();
    let status = local.catalog_status(&fixture.native).unwrap();
    assert_eq!(
        local
            .import_checked_profiles(
                &fixture.native,
                &status.revision,
                vec![IncomingProfile {
                    snapshot,
                    update_duplicate: true,
                    evidence: Some(report),
                    origin: IdentitySource::PackageDeclared
                }]
            )
            .unwrap(),
        vec![CaptureCommitOutcome::Refreshed]
    );
    assert!(!fixture.catalog().source_verified(&fixture.b));
    assert_eq!(
        fixture
            .store()
            .for_connection(ConnectionKind::Start, "3.0.0")
            .switch(&fixture.b),
        Err(TransactionError::UnverifiedSource)
    );
    assert!(!fixture.vault_root.path().join(JOURNAL_FILE).exists());
    assert_eq!(
        fixture
            .store()
            .for_connection(ConnectionKind::Coding, "3.14.4")
            .switch(&fixture.b)
            .unwrap(),
        SwitchOutcome::Switched
    );
}
#[tokio::test]
async fn failed_incoming_check_cannot_erase_existing_ready_complete_session() {
    use super::super::checkpoint::IdentitySource;
    let fixture = Fixture::new(OAuthFamily::Zai);
    let first = fixture
        .native
        .inspect(&native_document(OAuthFamily::Zai, "b", "ready-import"))
        .unwrap();
    let report = checked_report(&fixture.native, &first, true).await;
    let local = VaultAccountStore::new(fixture.vault_root.path(), &fixture.vault).unwrap();
    let status = local.catalog_status(&fixture.native).unwrap();
    local
        .import_checked_profiles(
            &fixture.native,
            &status.revision,
            vec![IncomingProfile {
                snapshot: first,
                update_duplicate: true,
                evidence: Some(report),
                origin: IdentitySource::PackageDeclared,
            }],
        )
        .unwrap();
    let before = fs::read(fixture.vault_root.path().join(PROFILE_FILE)).unwrap();
    let incoming = fixture
        .native
        .inspect(&native_document(OAuthFamily::Zai, "b", "failed-import"))
        .unwrap();
    let failed = checked_report(&fixture.native, &incoming, false).await;
    let status = local.catalog_status(&fixture.native).unwrap();
    assert_eq!(
        local
            .import_checked_profiles(
                &fixture.native,
                &status.revision,
                vec![IncomingProfile {
                    snapshot: incoming,
                    update_duplicate: true,
                    evidence: Some(failed),
                    origin: IdentitySource::PackageDeclared
                }]
            )
            .unwrap(),
        vec![CaptureCommitOutcome::Kept]
    );
    assert_eq!(
        before,
        fs::read(fixture.vault_root.path().join(PROFILE_FILE)).unwrap()
    );
}
#[tokio::test]
async fn native_recapture_preserves_nonce_stable_evidence_and_withdraws_only_changed_key() {
    use super::super::checkpoint::{ConnectionKind, IdentitySource};
    let fixture = Fixture::new(OAuthFamily::Zai);
    let document = native_document(OAuthFamily::Zai, "b", "ready-import");
    let snapshot = fixture.native.inspect(&document).unwrap();
    let report = checked_report(&fixture.native, &snapshot, true).await;
    let mut catalog = ProfileCatalog::default();
    assert!(catalog.upsert_checked(
        snapshot.clone(),
        &fixture.native,
        Some(report),
        IdentitySource::PackageDeclared
    ));
    catalog
        .set_label(
            snapshot.identity(),
            &fixture.native,
            Some("My local label".into()),
        )
        .unwrap();
    let mut values = std::collections::BTreeMap::new();
    for (name, value) in snapshot.scoped_document().entries() {
        values.insert(
            name.clone(),
            fixture
                .native
                .encrypt(&fixture.native.decrypt(value).unwrap())
                .unwrap(),
        );
    }
    let next = CredentialDocument::parse(&serde_json::to_vec(&values).unwrap()).unwrap();
    let recaptured = fixture.native.inspect(&next).unwrap();
    catalog.upsert(recaptured.clone());
    assert!(catalog.connection_ready(&recaptured, &fixture.native, ConnectionKind::Start));
    assert!(catalog.connection_ready(&recaptured, &fixture.native, ConnectionKind::Coding));
    let key = recaptured.identity().credential_keys()[5].clone();
    values.insert(
        key,
        fixture.native.encrypt("synthetic-changed-coding").unwrap(),
    );
    let changed = fixture
        .native
        .inspect(&CredentialDocument::parse(&serde_json::to_vec(&values).unwrap()).unwrap())
        .unwrap();
    catalog.upsert(changed.clone());
    let catalog = ProfileCatalog::open(
        &catalog.seal(&fixture.vault, &fixture.native).unwrap(),
        &fixture.vault,
        &fixture.native,
    )
    .unwrap();
    assert!(catalog.connection_ready(&changed, &fixture.native, ConnectionKind::Start));
    assert!(!catalog.connection_ready(&changed, &fixture.native, ConnectionKind::Coding));
    assert_eq!(
        catalog.details(&changed, &fixture.native).label.as_deref(),
        Some("My local label")
    );
}

#[tokio::test]
async fn review_native_recapture_does_not_bypass_v2_selected_capability_admission() {
    use super::super::checkpoint::{ConnectionKind, IdentitySource};
    let fixture = Fixture::new(OAuthFamily::Zai);
    let snapshot = fixture
        .native
        .inspect(&native_document(OAuthFamily::Zai, "b", "verified"))
        .unwrap();
    let good = checked_report(&fixture.native, &snapshot, true).await;
    let mut catalog = fixture.catalog();
    assert!(catalog.upsert_checked(
        snapshot.clone(),
        &fixture.native,
        Some(good),
        IdentitySource::PackageDeclared
    ));
    let mut values = snapshot.scoped_document().entries().clone();
    values.remove(&snapshot.identity().credential_keys()[5]);
    let captured = fixture
        .native
        .inspect(&CredentialDocument::parse(&serde_json::to_vec(&values).unwrap()).unwrap())
        .unwrap();
    catalog.upsert(captured.clone());
    assert!(catalog.source_verified(captured.identity()));
    write_durable(
        &fixture.vault_root.path().join(PROFILE_FILE),
        catalog
            .seal(&fixture.vault, &fixture.native)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        fixture
            .store()
            .for_connection(ConnectionKind::Coding, "3.14.4")
            .switch(&fixture.b),
        Err(TransactionError::UnverifiedSource)
    );
    assert!(!fixture.vault_root.path().join(JOURNAL_FILE).exists());
}

#[test]
fn review_legacy_native_record_label_edit_keeps_existing_activation_contract() {
    let fixture = Fixture::new(OAuthFamily::Zai);
    let mut catalog = fixture.catalog();
    catalog
        .set_label(&fixture.b, &fixture.native, Some("Local name".into()))
        .unwrap();
    write_durable(
        &fixture.vault_root.path().join(PROFILE_FILE),
        catalog
            .seal(&fixture.vault, &fixture.native)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        fixture.store().switch(&fixture.b).unwrap(),
        SwitchOutcome::Switched
    );
}

#[test]
fn io_key_cleanup_retry_is_noop_when_exact_unsubmitted_request_is_already_absent() {
    let temp = fs::canonicalize(std::env::temp_dir()).unwrap();
    let root = tempfile::tempdir_in(temp).unwrap();
    crate::config_file_io::ensure_private_directory(root.path()).unwrap();
    let vault = VaultContext::generate().unwrap();
    let store = VaultAccountStore::new(root.path(), &vault).unwrap();
    let a = KeyScope::new(OAuthFamily::BigModel, "cache-a", "org", "project-a").unwrap();
    let b = KeyScope::new(OAuthFamily::BigModel, "cache-b", "org", "project-b").unwrap();
    let Reservation::Fresh(old_a) = store.reserve_key_intent(a.clone()).unwrap() else {
        panic!()
    };
    let Reservation::Fresh(_b) = store.reserve_key_intent(b.clone()).unwrap() else {
        panic!()
    };
    store.clear_unsubmitted_key(&old_a).unwrap();
    let path = root
        .path()
        .join(crate::secrets::owned_file::KEY_INTENT_FILE);
    let only_b = fs::read(&path).unwrap();
    assert!(store.clear_unsubmitted_key(&old_a).is_ok());
    assert_eq!(fs::read(&path).unwrap(), only_b);
    let Reservation::Fresh(new_a) = store.reserve_key_intent(a.clone()).unwrap() else {
        panic!()
    };
    store.mark_key_created(&new_a).unwrap();
    let with_new_a = fs::read(&path).unwrap();
    assert!(store.clear_unsubmitted_key(&old_a).is_ok());
    assert_eq!(fs::read(&path).unwrap(), with_new_a);
    assert!(store.clear_unsubmitted_key(&new_a).is_err());
    assert_eq!(fs::read(&path).unwrap(), with_new_a);
    assert!(store.key_intent(&b).unwrap().is_some());
}
#[test]
fn io_key_cleanup_retry_is_noop_when_exact_copied_request_is_already_absent() {
    let temp = fs::canonicalize(std::env::temp_dir()).unwrap();
    let root = tempfile::tempdir_in(temp).unwrap();
    crate::config_file_io::ensure_private_directory(root.path()).unwrap();
    let vault = VaultContext::generate().unwrap();
    let store = VaultAccountStore::new(root.path(), &vault).unwrap();
    let a = KeyScope::new(OAuthFamily::BigModel, "cache-a", "org", "project-a").unwrap();
    let b = KeyScope::new(OAuthFamily::BigModel, "cache-b", "org", "project-b").unwrap();
    let Reservation::Fresh(old_a) = store.reserve_key_intent(a.clone()).unwrap() else {
        panic!()
    };
    store.reserve_key_intent(b.clone()).unwrap();
    store.mark_key_created(&old_a).unwrap();
    let receipt = store.key_intent(&a).unwrap().unwrap();
    store.clear_resolved_key(&receipt).unwrap();
    let path = root
        .path()
        .join(crate::secrets::owned_file::KEY_INTENT_FILE);
    let only_b = fs::read(&path).unwrap();
    assert!(store.clear_resolved_key(&receipt).is_ok());
    assert_eq!(fs::read(&path).unwrap(), only_b);
    store.reserve_key_intent(a.clone()).unwrap();
    let with_new_a = fs::read(&path).unwrap();
    assert!(store.clear_resolved_key(&receipt).is_ok());
    assert_eq!(fs::read(&path).unwrap(), with_new_a);
    assert!(store.key_intent(&a).unwrap().is_some());
    assert!(store.key_intent(&b).unwrap().is_some());
}

#[test]
fn io_library_labels_are_local_atomic_and_require_current_catalog() {
    let f = Fixture::new(OAuthFamily::Zai);
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let before = store.catalog_status(&f.native).unwrap();
    let native_before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    store
        .set_profile_label(
            &f.native,
            &before.revision,
            &f.a.opaque_id(),
            Some("Synthetic personal account".into()),
        )
        .unwrap();
    let after = store.catalog_status(&f.native).unwrap();
    assert_ne!(before.revision, after.revision);
    assert_eq!(
        after
            .profiles
            .iter()
            .find(|row| row.id == f.a.opaque_id())
            .unwrap()
            .label
            .as_deref(),
        Some("Synthetic personal account")
    );
    assert!(f.catalog().source_verified(&f.a));
    assert_eq!(
        native_before,
        fs::read(f.native_root.path().join("credentials.json")).unwrap()
    );
    let bytes = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("Synthetic personal account"));
    assert_eq!(
        store.set_profile_label(
            &f.native,
            &before.revision,
            &f.a.opaque_id(),
            Some("stale".into())
        ),
        Err(TransactionError::CatalogChanged)
    );
    assert_eq!(
        bytes,
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap()
    );
    store
        .set_profile_label(&f.native, &after.revision, &f.a.opaque_id(), None)
        .unwrap();
    assert_eq!(
        f.catalog()
            .details(f.catalog().get(&f.a).unwrap(), &f.native)
            .label,
        None
    );
}

#[test]
fn io_library_activation_is_granted_only_for_matching_admitted_native_selection() {
    let f = Fixture::new(OAuthFamily::Zai);
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let library = store.catalog_status(&f.native).unwrap();
    assert!(
        library.actions.can_add
            && library.actions.can_import
            && library.actions.can_backup
            && library.actions.can_edit_labels
    );
    assert!(library
        .profiles
        .iter()
        .all(|row| !row.can_activate && row.activation_blocked_reason.is_some()));
    let native = store
        .catalog_status_for_connection(
            &f.native,
            OAuthFamily::Zai,
            super::super::checkpoint::ConnectionKind::Coding,
            "3.14.4",
        )
        .unwrap();
    assert!(native
        .profiles
        .iter()
        .all(|row| row.can_activate && row.activation_blocked_reason.is_none()));
    let wrong_family = store
        .catalog_status_for_connection(
            &f.native,
            OAuthFamily::BigModel,
            super::super::checkpoint::ConnectionKind::Coding,
            "3.14.4",
        )
        .unwrap();
    assert!(wrong_family.profiles.iter().all(|row| !row.can_activate));
    assert!(library.current.is_none() && native.current.is_none());
}

#[test]
fn io_explicit_current_identity_reads_without_capture_or_catalog_mutation() {
    let f = Fixture::new(OAuthFamily::Zai);
    let catalog_before = fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap();
    let credentials_before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let snapshot = read_current_profile(
        f.native_root.path(),
        native_root_identity(f.native_root.path()).unwrap(),
        &f.native,
        OAuthFamily::Zai,
        &|| Ok(()),
    )
    .unwrap()
    .unwrap();
    assert!(snapshot.identity() == &f.a);
    assert_eq!(
        catalog_before,
        fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap()
    );
    assert_eq!(
        credentials_before,
        fs::read(f.native_root.path().join("credentials.json")).unwrap()
    );
    assert!(!f.vault_root.path().join(JOURNAL_FILE).exists());
}
#[test]
fn io_explicit_current_identity_rejects_late_source_change() {
    let f = Fixture::new(OAuthFamily::Zai);
    let root = f.native_root.path();
    let gate = || {
        write_durable(
            &root.join("credentials.json"),
            &native_document(OAuthFamily::Zai, "b", "changed")
                .to_bytes()
                .unwrap(),
        )
        .unwrap();
        Ok(())
    };
    assert!(matches!(
        read_current_profile(
            root,
            native_root_identity(root).unwrap(),
            &f.native,
            OAuthFamily::Zai,
            &gate
        ),
        Err(TransactionError::SourceChanged)
    ));
}

#[tokio::test]
async fn io_checked_bundle_preserves_whole_session_and_allows_coding_without_business_identity_proof(
) {
    use super::super::capture_reviews::Binding;
    use super::super::import_reviews::{ImportChoice, Reviews};
    use super::super::official::*;
    use std::sync::Mutex;
    struct OfficialFixture(Mutex<Vec<OfficialMethod>>);
    impl OfficialTransport for OfficialFixture {
        fn send(&self, request: OfficialRequest) -> TransportFuture<'_> {
            self.0.lock().unwrap().push(request.method);
            let (status, data) = if request.url.contains("subscription") {
                (
                    200,
                    serde_json::json!([{"productId":"coding-pro","status":"VALID","inCurrentPeriod":true}]),
                )
            } else if request.url.contains("quota") {
                (
                    200,
                    serde_json::json!({"limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"usage":0,"currentValue":0,"remaining":0,"percentage":0}]}),
                )
            } else {
                (401, serde_json::Value::Null)
            };
            Box::pin(async move {
                Ok(OfficialResponse {
                    status,
                    body: zeroize::Zeroizing::new(
                        serde_json::to_vec(&serde_json::json!({"code":0,"data":data})).unwrap(),
                    ),
                })
            })
        }
    }
    let f = Fixture::new(OAuthFamily::Zai);
    let store = VaultAccountStore::new(f.vault_root.path(), &f.vault).unwrap();
    let status = store.catalog_status(&f.native).unwrap();
    let catalog = store.import_catalog(&f.native, &status.revision).unwrap();
    let snapshot = f
        .native
        .inspect(&native_document(
            OAuthFamily::Zai,
            "import-ready",
            "whole-session",
        ))
        .unwrap();
    let bytes = snapshot.scoped_document().to_bytes().unwrap();
    let bundle = super::super::bundle::encode_bundle(
        &[super::super::bundle::BundleExportAccount {
            snapshot: &snapshot,
            created_at: "2026-10-07T00:00:00Z",
        }],
        &f.native,
        "synthetic-password",
        "2026-10-07T00:00:00Z",
    )
    .unwrap();
    let inspected =
        super::super::bundle_import::inspect(&bundle, "synthetic-password", &f.native, &catalog)
            .unwrap();
    let binding = Binding {
        vault_root: f.vault_root.path().into(),
        vault_id: "synthetic-vault".into(),
        key_id: "key".into(),
        vault_revision: 1,
        context_revision: "library".into(),
        catalog_revision: status.revision.clone(),
    };
    let now = std::time::Instant::now();
    let mut reviews = Reviews::default();
    let id = reviews.issue(binding.clone(), inspected.accounts, now);
    let selected = vec![ImportChoice {
        index: 0,
        update_duplicate: false,
    }];
    let lease = reviews
        .begin_check(&id, &binding, selected.clone(), now)
        .unwrap();
    let client = OfficialClient::new(OfficialFixture(Mutex::new(vec![])));
    let report = super::super::session_checks::check_session(
        &client,
        &f.native,
        &lease.accounts[0].1,
        None,
        42,
        &|| lease.check(),
    )
    .await
    .unwrap();
    assert_ne!(
        report.display().business.check.state,
        super::super::session_checks::CheckState::Accepted
    );
    assert_eq!(
        report.display().coding.entitlement,
        super::super::session_checks::EntitlementState::Available
    );
    reviews.finish_check(&lease, vec![(0, report)]).unwrap();
    let progress = reviews.check_progress(&id, now).unwrap();
    assert_eq!(progress.selected, selected);
    assert_eq!(progress.status, "ready");
    let mut changed = selected.clone();
    changed[0].update_duplicate = true;
    assert!(reviews
        .consume_checked(&id, &binding, &changed, now)
        .is_none());
    let (mut review, mut reports) = reviews
        .consume_checked(&id, &binding, &selected, now)
        .unwrap();
    let native_before = fs::read(f.native_root.path().join("credentials.json")).unwrap();
    let outcome = store
        .import_checked_profiles(
            &f.native,
            &status.revision,
            vec![IncomingProfile {
                snapshot: review.accounts[0].take().unwrap(),
                update_duplicate: false,
                evidence: reports.remove(&0),
                origin: super::super::checkpoint::IdentitySource::PackageDeclared,
            }],
        )
        .unwrap();
    assert_eq!(outcome, vec![CaptureCommitOutcome::Saved]);
    let updated = f.catalog();
    let saved = updated.get(snapshot.identity()).unwrap();
    assert_eq!(saved.scoped_document().to_bytes().unwrap(), bytes);
    assert_eq!(
        updated.details(saved, &f.native).identity_source,
        super::super::checkpoint::IdentitySource::PackageDeclared
    );
    assert!(!updated.source_verified(saved.identity()));
    assert!(updated.can_activate(
        saved,
        &f.native,
        Some((super::super::checkpoint::ConnectionKind::Coding, "3.14.4"))
    ));
    assert_eq!(
        native_before,
        fs::read(f.native_root.path().join("credentials.json")).unwrap()
    );
    assert!(client
        .transport
        .0
        .lock()
        .unwrap()
        .iter()
        .all(|method| *method == OfficialMethod::Get));
}
