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
fn io_precommit_failure_restores_target_preimages_and_keeps_unrelated_changes() {
    let f = Fixture::new(OAuthFamily::Zai);
    assert!(f
        .store()
        .switch_with_hook(&f.b, &mut |p| if p == WritePoint::NativeCredentials {
            Err(TransactionError::Storage)
        } else {
            Ok(())
        })
        .is_err());
    let altered = replace(&f.current(), "future:unrelated", "after-failure".into());
    write_durable(
        &f.native_root.path().join("credentials.json"),
        &altered.to_bytes().unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.store().recover(JournalOrigin::Live),
        Ok(SwitchOutcome::Recovered)
    );
    for key in f.a.credential_keys() {
        assert_eq!(f.current().get(&key), f.fresh.get(&key));
    }
    for key in
        f.b.credential_keys()
            .into_iter()
            .filter(|key| key.starts_with("account-provider:"))
    {
        assert!(f.current().get(&key).is_none());
    }
    assert_eq!(f.current().get("future:unrelated"), Some("after-failure"));
}
#[test]
fn io_committed_or_imported_journals_cannot_roll_back_native_account() {
    let f = Fixture::new(OAuthFamily::Zai);
    assert!(f
        .store()
        .switch_with_hook(&f.b, &mut |p| if p == WritePoint::Committed {
            Err(TransactionError::Storage)
        } else {
            Ok(())
        })
        .is_err());
    let current = f.current();
    assert_eq!(
        f.store().recover(JournalOrigin::Restored),
        Err(TransactionError::Imported)
    );
    assert!(f.current() == current);
    assert_eq!(
        f.store().recover(JournalOrigin::Live),
        Ok(SwitchOutcome::Recovered)
    );
    assert!(f.current() == current);
    assert_eq!(
        f.store().recover(JournalOrigin::Live),
        Ok(SwitchOutcome::NothingPending)
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
        let result = reopened.recover(JournalOrigin::Live).unwrap();
        assert_eq!(
            result,
            if point == WritePoint::JournalRemoved {
                SwitchOutcome::NothingPending
            } else {
                SwitchOutcome::Recovered
            }
        );
        let expected = if index < 4 { &f.a } else { &f.b };
        assert!(
            f.native.inspect(&f.current()).unwrap().identity() == expected,
            "crash point {point:?}"
        );
        assert!(
            f.catalog().get(&f.a).unwrap().scoped_document()
                == f.native.inspect(&f.fresh).unwrap().scoped_document()
        );
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
        let outcome = f.store().recover(JournalOrigin::Live).unwrap();
        assert_eq!(
            outcome,
            if point == WritePoint::JournalRemoved {
                SwitchOutcome::NothingPending
            } else {
                SwitchOutcome::Recovered
            }
        );
        let committed = matches!(
            point,
            WritePoint::Committed | WritePoint::RecoveryRecord | WritePoint::JournalRemoved
        );
        assert!(
            f.native.inspect(&f.current()).unwrap().identity()
                == if committed { &f.b } else { &f.a }
        );
    }
}
#[test]
fn io_conflicting_new_source_profile_or_touched_native_key_preserves_pending_journal() {
    for change_profile in [false, true] {
        let f = Fixture::new(OAuthFamily::Zai);
        assert!(f
            .store()
            .switch_with_hook(&f.b, &mut |p| if p == WritePoint::NativeCredentials {
                Err(TransactionError::Storage)
            } else {
                Ok(())
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
        assert!(f.store().recover(JournalOrigin::Live).is_err());
        assert_eq!(
            fs::read(f.native_root.path().join("credentials.json")).unwrap(),
            current
        );
        assert_eq!(
            fs::read(f.vault_root.path().join(PROFILE_FILE)).unwrap(),
            profiles
        );
        assert!(f.vault_root.path().join(JOURNAL_FILE).exists());
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
    f.store().recover(JournalOrigin::Live).unwrap();
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
        f.store().recover(JournalOrigin::Live).unwrap();
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
    store.recover(JournalOrigin::Live).unwrap();
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
    store.recover(JournalOrigin::Live).unwrap();
    assert!(f.current() == current);
}
#[test]
fn io_committed_recovery_cleanup_failure_keeps_commit_classification() {
    let f = Fixture::new(OAuthFamily::Zai);
    let interrupted = f.store().switch_with_hook(&f.b, &mut |point| {
        if point == WritePoint::Committed {
            return Err(TransactionError::Storage);
        }
        Ok(())
    });
    assert_eq!(interrupted, Err(TransactionError::CommittedNeedsCleanup));
    let current = f.current();
    let calls = std::cell::Cell::new(0);
    let gate = || {
        calls.set(calls.get() + 1);
        if calls.get() == 4 {
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
    assert_eq!(
        store.recover(JournalOrigin::Live),
        Err(TransactionError::CommittedNeedsCleanup)
    );
    assert!(f.current() == current);
    assert!(f.vault_root.path().join(JOURNAL_FILE).exists());
    store.recover(JournalOrigin::Live).unwrap();
    assert!(f.current() == current);
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
    assert!(f.store().status().unwrap().pending);
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
