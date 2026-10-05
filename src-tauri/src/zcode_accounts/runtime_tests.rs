use super::super::admission::{BuildFingerprint, KeyMode, Platform, VerifiedContext, WriterState};
use super::super::checkpoint::{ProfileCatalog, PROFILE_FILE};
use super::super::core::{CredentialDocument, OAuthFamily};
use super::super::native::tests::native_document_with_context;
use super::super::recovery::{RecoveryError, RecoveryLedger};
use super::*;
use crate::config_file_io::{ensure_private_directory, write_durable};
use crate::secrets::{
    owned_file::{JOURNAL_FILE, RECOVERY_FILE},
    session::SecretSession,
    testing::MemoryKeyStore,
};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

// Test-only sealed export; uses synthetic credential documents and never a path.
fn synthetic_bundle(f: &Fixture, count: usize) -> Zeroizing<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    let credentials: serde_json::Value =
        serde_json::from_slice(&f.fresh.to_bytes().unwrap()).unwrap();
    let plaintext = Zeroizing::new(serde_json::to_vec(&serde_json::json!({
        "format":"zcode-accounts-bundle", "version":2, "exportedAt":"synthetic",
        "accounts": (0..count).map(|_| serde_json::json!({"name":"synthetic", "createdAt":"synthetic", "credentials":credentials, "config":{"device":"MUST_NOT_IMPORT"}})).collect::<Vec<_>>()
    })).unwrap());
    let mut key = Zeroizing::new([0u8; 32]);
    let salt = [3u8; 16];
    let nonce = [5u8; 12];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(100_000).unwrap(),
        &salt,
        b"synthetic-password",
        &mut *key,
    );
    let encrypted = Aes256Gcm::new_from_slice(&*key)
        .unwrap()
        .encrypt(&Nonce::from(nonce), plaintext.as_slice())
        .unwrap();
    let (data, tag) = encrypted.split_at(encrypted.len() - 16);
    Zeroizing::new(serde_json::to_vec(&serde_json::json!({"format":"zsw-accounts-bundle","version":1,"kdf":{"algo":"pbkdf2-hmac-sha256","iters":100000,"salt":STANDARD.encode(salt)},"cipher":{"algo":"aes-256-gcm","nonce":STANDARD.encode(nonce),"tag":STANDARD.encode(tag),"data":STANDARD.encode(data)}})).unwrap())
}
async fn import_preview(f: &Fixture, revision: String, count: usize) -> BundlePreview {
    preview_bundle(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        BundlePreviewInput {
            revision,
            file: synthetic_bundle(f, count),
            password: Zeroizing::new("synthetic-password".into()),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_bundle_vault_only_keep_update_and_replay_never_read_native_credentials() {
    let f = Fixture::new();
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    std::fs::remove_file(f.native_root.join("credentials.json")).unwrap();
    let before_files = std::fs::read_dir(&f.native_root)
        .unwrap()
        .map(|p| p.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    let p = import_preview(&f, status.revision.clone(), 1).await;
    let safe = serde_json::to_string(&p).unwrap();
    for forbidden in [
        "MUST_NOT_IMPORT",
        "token",
        "credentials",
        "nativeRevision",
        "enc:v1:",
    ] {
        assert!(!safe.contains(forbidden));
    }
    let before = std::fs::read(f.db.secret_session().root().join(PROFILE_FILE)).unwrap();
    let input = || BundleCommitInput {
        revision: status.revision.clone(),
        preview_id: p.preview_id.clone(),
        selected: vec![ImportChoice {
            index: 0,
            update_duplicate: false,
        }],
    };
    assert_eq!(
        commit_bundle(f.db.clone(), f.probe.clone(), f.contracts.clone(), input()).await,
        Ok(vec![CaptureCommitOutcome::Kept])
    );
    assert_eq!(
        commit_bundle(f.db.clone(), f.probe.clone(), f.contracts.clone(), input()).await,
        Err(RuntimeError::Transaction(TransactionError::SourceChanged))
    );
    assert_eq!(
        std::fs::read(f.db.secret_session().root().join(PROFILE_FILE)).unwrap(),
        before
    );
    let p = import_preview(&f, status.revision.clone(), 1).await;
    assert_eq!(
        commit_bundle(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            BundleCommitInput {
                revision: status.revision,
                preview_id: p.preview_id,
                selected: vec![ImportChoice {
                    index: 0,
                    update_duplicate: true
                }]
            }
        )
        .await,
        Ok(vec![CaptureCommitOutcome::Refreshed])
    );
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    assert!(
        !status
            .profiles
            .iter()
            .find(|p| p.id == f.a.opaque_id())
            .unwrap()
            .source_verified
    );
    assert_eq!(
        std::fs::read_dir(&f.native_root)
            .unwrap()
            .map(|p| p.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>(),
        before_files
    );
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_bundle_failed_admission_and_duplicate_selection_consume_preview() {
    let f = Fixture::new();
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    let before = std::fs::read(f.db.secret_session().root().join(PROFILE_FILE)).unwrap();
    let p = import_preview(&f, status.revision.clone(), 2).await;
    assert!(p.rows.iter().all(|row| row.ambiguous));
    let id = p.preview_id;
    let input = |indices: &[usize]| BundleCommitInput {
        revision: status.revision.clone(),
        preview_id: id.clone(),
        selected: indices
            .iter()
            .map(|index| ImportChoice {
                index: *index,
                update_duplicate: true,
            })
            .collect(),
    };
    assert!(commit_bundle(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        input(&[0, 1])
    )
    .await
    .is_err());
    assert!(commit_bundle(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        input(&[1])
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read(f.db.secret_session().root().join(PROFILE_FILE)).unwrap(),
        before
    );
    let p = import_preview(&f, status.revision.clone(), 1).await;
    f.probe.observation.lock().unwrap().writers = WriterState::Running;
    assert_eq!(
        commit_bundle(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            BundleCommitInput {
                revision: status.revision.clone(),
                preview_id: p.preview_id.clone(),
                selected: vec![ImportChoice {
                    index: 0,
                    update_duplicate: true
                }]
            }
        )
        .await,
        Err(RuntimeError::Blocked(BlockedReason::AppRunning))
    );
    f.probe.observation.lock().unwrap().writers = WriterState::Stopped;
    assert!(commit_bundle(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        BundleCommitInput {
            revision: status.revision,
            preview_id: p.preview_id,
            selected: vec![ImportChoice {
                index: 0,
                update_duplicate: true
            }]
        }
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read(f.db.secret_session().root().join(PROFILE_FILE)).unwrap(),
        before
    );
}

tokio::task_local! {
    pub(super) static OWNER_ENTERED: Arc<tokio::sync::Notify>;
}

struct FakeProbe {
    observation: Mutex<ContextObservation>,
    calls: AtomicUsize,
    pause: Mutex<Option<(usize, std::sync::mpsc::Receiver<()>)>>,
    fail_at: AtomicUsize,
    entered: tokio::sync::Notify,
}
struct CoordinatedProbe {
    inner: Arc<FakeProbe>,
    quits: AtomicUsize,
    restarts: AtomicUsize,
    remains_running: bool,
    restart_fails: bool,
}
impl ContextProbe for CoordinatedProbe {
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        self.inner.observe()
    }
    fn prepare_switch(&self) -> Result<bool, BlockedReason> {
        assert!(crate::services::sync_protocol::sync_mutex()
            .try_lock()
            .is_err());
        self.quits.fetch_add(1, Ordering::Relaxed);
        if !self.remains_running {
            self.inner.observation.lock().unwrap().writers = WriterState::Stopped;
        }
        Ok(true)
    }
    fn restart_after_switch(&self) -> Result<(), BlockedReason> {
        assert!(crate::services::sync_protocol::sync_mutex()
            .try_lock()
            .is_err());
        self.restarts.fetch_add(1, Ordering::Relaxed);
        if self.restart_fails {
            return Err(BlockedReason::WriterStateUnknown);
        }
        self.inner.observation.lock().unwrap().writers = WriterState::Running;
        Ok(())
    }
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_coordinated_switch_checks_stopped_proof_and_preserves_commit_on_restart_failure() {
    for (remains_running, restart_fails) in [(true, false), (false, true), (false, false)] {
        let f = Fixture::new();
        f.probe.observation.lock().unwrap().writers = WriterState::Running;
        let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        let before = f.current().to_bytes().unwrap();
        let probe = Arc::new(CoordinatedProbe {
            inner: f.probe.clone(),
            quits: AtomicUsize::new(0),
            restarts: AtomicUsize::new(0),
            remains_running,
            restart_fails,
        });
        let request_id = uuid::Uuid::new_v4().to_string();
        let result = switch_account_request(
            f.db.clone(),
            probe.clone(),
            f.contracts.clone(),
            f.b.opaque_id(),
            status.revision.clone(),
            request_id.clone(),
        )
        .await;
        let original = operation_status(
            f.db.clone(),
            probe.clone(),
            f.contracts.clone(),
            request_id.clone(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            original.phase,
            if remains_running {
                super::super::operation_log::Phase::Failed
            } else if restart_fails {
                super::super::operation_log::Phase::RestartFailed
            } else {
                super::super::operation_log::Phase::RestartVerified
            }
        );
        // Discarded/lost first response and repeated confirmation query the same
        // durable result; a repeated UUID never runs a second physical effect.
        assert_eq!(
            switch_account_request(
                f.db.clone(),
                probe.clone(),
                f.contracts.clone(),
                f.b.opaque_id(),
                status.revision,
                request_id
            )
            .await,
            Err(RuntimeError::Transaction(
                TransactionError::OperationAlreadyKnown
            ))
        );
        assert_eq!(probe.quits.load(Ordering::Relaxed), 1);
        if remains_running {
            assert_eq!(
                result,
                Err(RuntimeError::Blocked(BlockedReason::AppRunning))
            );
            assert_eq!(f.current().to_bytes().unwrap(), before);
            assert_eq!(probe.restarts.load(Ordering::Relaxed), 0);
        } else {
            assert_eq!(probe.restarts.load(Ordering::Relaxed), 1);
            assert_eq!(
                result,
                if restart_fails {
                    Err(RuntimeError::CommittedRestartFailed)
                } else {
                    Ok(SwitchOutcome::Switched)
                }
            );
            let observed = f.probe.observation.lock().unwrap().clone();
            let native = ReadOnlyContext::assess(observed, &f.contracts)
                .unwrap()
                .vault_cipher()
                .unwrap();
            assert!(native.inspect(&f.current()).unwrap().identity() == &f.b);
            assert!(!f.db.secret_session().root().join(JOURNAL_FILE).exists());
        }
    }
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_coordinated_switch_stale_catalog_refuses_before_quit() {
    let f = Fixture::new();
    f.probe.observation.lock().unwrap().writers = WriterState::Running;
    let probe = Arc::new(CoordinatedProbe {
        inner: f.probe.clone(),
        quits: AtomicUsize::new(0),
        restarts: AtomicUsize::new(0),
        remains_running: false,
        restart_fails: false,
    });
    assert_eq!(
        switch_saved_account(
            f.db.clone(),
            probe.clone(),
            f.contracts.clone(),
            f.b.opaque_id(),
            "stale".into()
        )
        .await,
        Err(RuntimeError::Transaction(TransactionError::CatalogChanged))
    );
    assert_eq!(probe.quits.load(Ordering::Relaxed), 0);
    assert_eq!(probe.restarts.load(Ordering::Relaxed), 0);
}
impl ContextProbe for FakeProbe {
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        assert!(
            crate::services::sync_protocol::sync_mutex()
                .try_lock()
                .is_err(),
            "physical operation must own existing sync mutex"
        );
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let pause = {
            let mut pending = self.pause.lock().unwrap();
            if pending.as_ref().is_some_and(|(at, _)| *at == call) {
                pending.take().map(|(_, receiver)| receiver)
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            self.entered.notify_one();
            pause
                .recv()
                .map_err(|_| BlockedReason::WriterStateUnknown)?;
        }
        if self.fail_at.load(Ordering::Relaxed) == call {
            return Err(BlockedReason::AppRunning);
        }
        Ok(self.observation.lock().unwrap().clone())
    }
}
struct Fixture {
    db: Arc<Database>,
    _store: MemoryKeyStore,
    probe: Arc<FakeProbe>,
    contracts: Vec<ContractEntry>,
    native_root: PathBuf,
    a: AccountIdentity,
    b: AccountIdentity,
    fresh: CredentialDocument,
    old_home: Option<std::ffi::OsString>,
    _temporary: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        Self::for_family(OAuthFamily::Zai)
    }
    fn for_family(family: OAuthFamily) -> Self {
        let temporary =
            tempfile::tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let home = temporary.path();
        ensure_private_directory(home).unwrap();
        let old_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", home);
        let root = home.join(crate::APP_DIR_NAME);
        let store = MemoryKeyStore::default();
        let session = SecretSession::open(&root, &store, None).unwrap();
        let conn = crate::database::vault::prepare(
            &root.join(crate::config::DB_FILE_NAME),
            &session.read().unwrap(),
        )
        .unwrap();
        let db = Arc::new(Database::from_connection(conn, session.clone()));
        session.complete_migration().unwrap();
        let data = home.join("native-data");
        ensure_private_directory(&data).unwrap();
        ensure_private_directory(&data.join(".zcode")).unwrap();
        let native_root = data.join(".zcode/v2");
        ensure_private_directory(&native_root).unwrap();
        let install = BuildFingerprint {
            platform: Platform::MacOs,
            version: "synthetic-version".into(),
            build: "synthetic-build".into(),
            artifact_sha256: [7; 32],
        };
        let contracts = vec![ContractEntry {
            fingerprint: install.clone(),
            native_gate_passed: true,
        }];
        let home_text = home.to_str().unwrap().to_owned();
        let metadata = std::fs::metadata(&native_root).unwrap();
        let family_name = match family {
            OAuthFamily::Zai => "zai",
            OAuthFamily::BigModel => "bigmodel",
        };
        let observation = ContextObservation {
            install,
            credential_root: native_root.clone(),
            settings_file: home.join(".zcode/v2/setting.json"),
            root_identity: [metadata.dev(), metadata.ino()],
            settings_identity: [1, 3],
            home: home_text.clone(),
            settings_home: home_text.clone(),
            bootstrap_home: home_text.clone(),
            username: "synthetic-user".into(),
            key_choice: KeyMode::Standard,
            writers: WriterState::Stopped,
            settings: serde_json::to_vec(&serde_json::json!({
                "dataBaseDir": data,
                "providerFamilyDomain": family_name,
                "providerFamilyConnectionSelections": {family_name: {"kind": "individual-coding-plan"}}
            }))
            .unwrap(),
        };
        let context = VerifiedContext::assess(observation.clone(), &contracts).unwrap();
        let secret = zeroize::Zeroizing::new(format!(
            "zcode-credential-fallback:darwin:{home_text}:synthetic-user"
        ));
        let native = context.cipher().unwrap();
        let old = native_document_with_context(context.context_id(), &secret, family, "a", "old");
        let fresh =
            native_document_with_context(context.context_id(), &secret, family, "a", "fresh");
        let target =
            native_document_with_context(context.context_id(), &secret, family, "b", "saved");
        let a = native.inspect(&old).unwrap().identity().clone();
        let b = native.inspect(&target).unwrap().identity().clone();
        let mut catalog = ProfileCatalog::default();
        catalog.upsert(native.inspect(&old).unwrap());
        catalog.upsert(native.inspect(&target).unwrap());
        write_durable(
            &root.join(PROFILE_FILE),
            catalog
                .seal(&session.read().unwrap(), &native)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        write_durable(
            &native_root.join("credentials.json"),
            &fresh.to_bytes().unwrap(),
        )
        .unwrap();
        let probe = Arc::new(FakeProbe {
            observation: Mutex::new(observation),
            calls: AtomicUsize::new(0),
            pause: Mutex::new(None),
            fail_at: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
        });
        Self {
            db,
            _store: store,
            probe,
            contracts,
            native_root,
            a,
            b,
            fresh,
            old_home,
            _temporary: temporary,
        }
    }
    async fn interrupt_before_native(&self) {
        // Admission1, constructor2, lock3/4, prepared5, profile6, captured7;
        // refuse native publication at8, relative to earlier fixture operations.
        self.probe.fail_at.store(
            self.probe.calls.load(Ordering::Relaxed) + 8,
            Ordering::Relaxed,
        );
        assert_eq!(
            switch_account(
                self.db.clone(),
                self.probe.clone(),
                self.contracts.clone(),
                self.b.clone(),
            )
            .await,
            Err(RuntimeError::Transaction(TransactionError::Admission(
                BlockedReason::AppRunning
            )))
        );
        self.probe.fail_at.store(0, Ordering::Relaxed);
        assert!(self.db.secret_session().root().join(JOURNAL_FILE).exists());
        assert!(self.current() == self.fresh);
    }
    async fn archive_pending(&self) -> RecoveryStatus {
        let pending = recovery_status(self.db.clone()).await.unwrap();
        assert!(pending.pending && pending.native_unconfirmed);
        assert_eq!(
            archive_pending_recovery(self.db.clone(), pending.revision).await,
            Ok(ArchiveOutcome::Archived)
        );
        let archived = recovery_status(self.db.clone()).await.unwrap();
        assert!(!archived.pending && archived.native_unconfirmed);
        assert_eq!(archived.records.len(), 1);
        assert_eq!(archived.records[0].disposition, "native-unconfirmed");
        archived
    }
    fn recovery_payload(&self) -> Vec<u8> {
        let encoded =
            std::fs::read_to_string(self.db.secret_session().root().join(RECOVERY_FILE)).unwrap();
        let vault = self.db.secret_session().read().unwrap();
        let ledger = RecoveryLedger::open(&encoded, &vault).unwrap();
        let record = ledger.archived().next().unwrap();
        record.evidence().raw_payload().to_vec()
    }
    fn current(&self) -> CredentialDocument {
        CredentialDocument::parse(
            &std::fs::read(self.native_root.join("credentials.json")).unwrap(),
        )
        .unwrap()
    }
    fn native_login(&self, family: OAuthFamily, id: &str, version: &str) -> CredentialDocument {
        let context = VerifiedContext::assess(
            self.probe.observation.lock().unwrap().clone(),
            &self.contracts,
        )
        .unwrap();
        let home = self.probe.observation.lock().unwrap().home.clone();
        let secret = zeroize::Zeroizing::new(format!(
            "zcode-credential-fallback:darwin:{home}:synthetic-user"
        ));
        let document =
            native_document_with_context(context.context_id(), &secret, family, id, version);
        write_durable(
            &self.native_root.join("credentials.json"),
            &document.to_bytes().unwrap(),
        )
        .unwrap();
        document
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        match &self.old_home {
            Some(home) => std::env::set_var("CC_SWITCH_TEST_HOME", home),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_uses_existing_session_and_refreshes_a_b_a() {
    let f = Fixture::new();
    assert_eq!(
        switch_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.b.clone()
        )
        .await,
        Ok(SwitchOutcome::Switched)
    );
    assert_eq!(
        switch_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.a.clone()
        )
        .await,
        Ok(SwitchOutcome::Switched)
    );
    for key in f.a.credential_keys() {
        assert_eq!(f.current().get(&key), f.fresh.get(&key));
    }
    assert!(f.probe.calls.load(Ordering::Relaxed) >= 8);
}
#[tokio::test]
#[serial_test::serial]
async fn runtime_rejects_unverified_context_before_native_credentials_are_read() {
    let f = Fixture::new();
    std::fs::remove_file(f.native_root.join("credentials.json")).unwrap();
    f.probe.observation.lock().unwrap().install.artifact_sha256 = [9; 32];
    assert_eq!(
        switch_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.b.clone()
        )
        .await,
        Err(RuntimeError::Blocked(BlockedReason::UnsupportedBuild))
    );
    assert!(!f.native_root.join("credentials.json").exists());
    {
        let mut observation = f.probe.observation.lock().unwrap();
        observation.install.artifact_sha256 = [7; 32];
        observation.root_identity[1] ^= 1;
    }
    assert_eq!(
        switch_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.b.clone()
        )
        .await,
        Err(RuntimeError::Blocked(BlockedReason::ContextChanged))
    );
    assert!(!f.native_root.join("credentials.json").exists());
}
#[tokio::test]
#[serial_test::serial]
async fn runtime_rejects_blocked_vault_for_native_and_local_recovery_operations() {
    let f = Fixture::new();
    f.db.secrets.set_blocked(true);
    assert_eq!(
        switch_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.b.clone()
        )
        .await,
        Err(RuntimeError::VaultUnavailable)
    );
    assert!(matches!(
        recovery_status(f.db.clone()).await,
        Err(RuntimeError::VaultUnavailable)
    ));
    assert_eq!(
        archive_pending_recovery(f.db.clone(), "unused revision".into()).await,
        Err(RuntimeError::VaultUnavailable)
    );
    assert_eq!(
        delete_confirmed_recovery(f.db.clone(), "unused id".into(), "unused revision".into()).await,
        Err(RuntimeError::VaultUnavailable)
    );
    f.db.secrets.set_blocked(false);
    assert!(f.current() == f.fresh);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn runtime_caller_cancellation_does_not_release_owner_before_blocking_work_finishes() {
    let f = Fixture::new();
    let (release, receive) = std::sync::mpsc::channel();
    *f.probe.pause.lock().unwrap() = Some((1, receive));
    let task = tokio::spawn(switch_account(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        f.b.clone(),
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.probe.entered.notified(),
    )
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(crate::services::sync_protocol::sync_mutex()
        .try_lock()
        .is_err());
    release.send(()).unwrap();
    let _guard = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::services::sync_protocol::sync_mutex().lock(),
    )
    .await
    .unwrap();
    let context =
        VerifiedContext::assess(f.probe.observation.lock().unwrap().clone(), &f.contracts).unwrap();
    assert!(
        context
            .cipher()
            .unwrap()
            .inspect(&f.current())
            .unwrap()
            .identity()
            == &f.b
    );
    assert!(!f
        .db
        .secret_session()
        .root()
        .join(crate::secrets::owned_file::JOURNAL_FILE)
        .exists());
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_explicit_capture_and_saved_id_switch_share_the_existing_owner() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let f = Fixture::for_family(family);
        std::fs::remove_file(f.db.secret_session().root().join(PROFILE_FILE)).unwrap();
        let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert!(status.profiles.is_empty() && status.current.is_none());
        assert_eq!(
            capture_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                status.revision.clone()
            )
            .await,
            Ok(CaptureOutcome::Saved)
        );
        assert_eq!(
            capture_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                status.revision
            )
            .await,
            Err(RuntimeError::Transaction(TransactionError::CatalogChanged))
        );
        f.native_login(family, "b", "fresh-b");
        let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert_eq!(
            capture_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                status.revision
            )
            .await,
            Ok(CaptureOutcome::Saved)
        );
        let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert_eq!(status.profiles.len(), 2);
        assert_eq!(
            switch_saved_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                f.a.opaque_id(),
                status.revision
            )
            .await,
            Ok(SwitchOutcome::Switched)
        );
        let refreshed_a = f.native_login(family, "a", "refreshed-a");
        let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert_eq!(
            switch_saved_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                f.b.opaque_id(),
                status.revision
            )
            .await,
            Ok(SwitchOutcome::Switched)
        );
        let refreshed_b = f.native_login(family, "b", "refreshed-b");
        let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert_eq!(
            switch_saved_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                f.a.opaque_id(),
                status.revision
            )
            .await,
            Ok(SwitchOutcome::Switched)
        );
        for key in f.a.credential_keys() {
            assert_eq!(f.current().get(&key), refreshed_a.get(&key));
        }
        let context =
            VerifiedContext::assess(f.probe.observation.lock().unwrap().clone(), &f.contracts)
                .unwrap();
        let native = context.cipher().unwrap();
        let encoded =
            std::fs::read_to_string(f.db.secret_session().root().join(PROFILE_FILE)).unwrap();
        let catalog =
            ProfileCatalog::open(&encoded, &f.db.secret_session().read().unwrap(), &native)
                .unwrap();
        assert!(
            catalog.get(&f.b).unwrap().scoped_document()
                == native.inspect(&refreshed_b).unwrap().scoped_document()
        );
        std::fs::remove_file(f.native_root.join("credentials.json")).unwrap();
        let passive = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert_eq!(passive.profiles.len(), 2);
        assert!(passive.current.is_none() && !passive.pending);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn runtime_session_writer_waits_until_physical_account_io_finishes() {
    let f = Fixture::new();
    let (release, receive) = std::sync::mpsc::channel();
    // Probe2 is AccountStore construction, after the actual session read guard.
    *f.probe.pause.lock().unwrap() = Some((2, receive));
    let operation = tokio::spawn(switch_account(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        f.b.clone(),
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.probe.entered.notified(),
    )
    .await
    .unwrap();
    let writer_db = f.db.clone();
    let (started_send, started) = std::sync::mpsc::channel();
    let (acquired_send, acquired) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        started_send.send(()).unwrap();
        let _write = writer_db.secret_session().write().unwrap();
        acquired_send.send(()).unwrap();
    });
    started
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        acquired.recv_timeout(std::time::Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
    release.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), operation)
            .await
            .unwrap()
            .unwrap(),
        Ok(SwitchOutcome::Switched)
    );
    acquired
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    writer.join().unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_pending_transaction_blocks_maintenance_then_archive_preserves_disposition() {
    let f = Fixture::new();
    f.interrupt_before_native().await;
    let root = f.db.secret_session().root();
    let metadata = std::fs::read(root.join("vault.json")).unwrap();
    let profiles = std::fs::read(root.join(PROFILE_FILE)).unwrap();
    {
        let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
        for result in [
            crate::secrets::rewrap::change_password(
                &f.db,
                &f._store,
                "synthetic maintenance password",
                false,
            ),
            crate::secrets::transition::rotate(
                &f.db,
                &f._store,
                "synthetic maintenance password",
                false,
            ),
            crate::secrets::reset::reset(
                root,
                "synthetic expected revision",
                "synthetic maintenance password",
            )
            .map(|_| ()),
        ] {
            assert!(
                matches!(result, Err(crate::error::AppError::Config(code)) if code == "secret.zcode_recovery_required")
            );
        }
    }
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), metadata);
    assert_eq!(std::fs::read(root.join(PROFILE_FILE)).unwrap(), profiles);
    let archived = f.archive_pending().await;
    let original_payload = f.recovery_payload();
    let original_key =
        f.db.secret_session()
            .read()
            .unwrap()
            .metadata()
            .key_id
            .clone();
    for rotate in [false, true] {
        {
            let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
            if rotate {
                crate::secrets::transition::rotate(
                    &f.db,
                    &f._store,
                    "synthetic maintenance password",
                    false,
                )
                .unwrap();
            } else {
                crate::secrets::rewrap::change_password(
                    &f.db,
                    &f._store,
                    "synthetic maintenance password",
                    false,
                )
                .unwrap();
            }
            assert!(matches!(
                crate::secrets::reset::reset(root, "unused revision", "synthetic reset password"),
                Err(crate::error::AppError::Config(code)) if code == "secret.zcode_recovery_required"
            ));
        }
        let status = recovery_status(f.db.clone()).await.unwrap();
        assert!(!status.pending && status.native_unconfirmed);
        assert_eq!(status.records[0].id, archived.records[0].id);
        assert_eq!(status.records[0].disposition, "native-unconfirmed");
        assert_eq!(f.recovery_payload(), original_payload);
        let catalog = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert!(catalog.native_unconfirmed && !catalog.pending);
        assert_eq!(
            capture_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                catalog.revision.clone()
            )
            .await,
            Err(RuntimeError::Transaction(
                TransactionError::NativeUnconfirmed
            ))
        );
        assert_eq!(
            switch_saved_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                f.b.opaque_id(),
                catalog.revision
            )
            .await,
            Err(RuntimeError::Transaction(
                TransactionError::NativeUnconfirmed
            ))
        );
        assert_eq!(
            switch_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                f.b.clone()
            )
            .await,
            Err(RuntimeError::Transaction(
                TransactionError::NativeUnconfirmed
            ))
        );
        assert!(f.current() == f.fresh);
    }
    assert_ne!(
        f.db.secret_session().read().unwrap().metadata().key_id,
        original_key
    );
    let status = recovery_status(f.db.clone()).await.unwrap();
    assert_eq!(
        confirm_archived_recovery(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            status.records[0].id.clone(),
            status.revision
        )
        .await,
        Ok(())
    );
    let confirmed = recovery_status(f.db.clone()).await.unwrap();
    assert!(!confirmed.native_unconfirmed);
    assert_eq!(confirmed.records[0].disposition, "full-before");
    assert!(f.current() == f.fresh);
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    assert!(matches!(
        crate::secrets::reset::reset(root, "unused revision", "synthetic reset password"),
        Err(crate::error::AppError::Config(code)) if code == "secret.zcode_recovery_required"
    ));
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_vault_recovery_status_and_archive_need_no_native_context() {
    let f = Fixture::new();
    f.interrupt_before_native().await;
    let root = f.db.secret_session().root();
    let journal = std::fs::read(root.join(JOURNAL_FILE)).unwrap();
    let profiles = std::fs::read(root.join(PROFILE_FILE)).unwrap();
    let parked = f.native_root.with_file_name("unavailable-native-root");
    std::fs::rename(&f.native_root, &parked).unwrap();
    f.probe.observation.lock().unwrap().install.artifact_sha256 = [9; 32];
    let calls = f.probe.calls.load(Ordering::Relaxed);
    let status = recovery_status(f.db.clone()).await.unwrap();
    assert!(status.pending && status.native_unconfirmed);
    assert_eq!(std::fs::read(root.join(JOURNAL_FILE)).unwrap(), journal);
    assert_eq!(
        archive_pending_recovery(f.db.clone(), "stale revision".into()).await,
        Err(RuntimeError::Transaction(TransactionError::RecoveryChanged))
    );
    assert_eq!(std::fs::read(root.join(JOURNAL_FILE)).unwrap(), journal);
    let archived = f.archive_pending().await;
    assert_eq!(
        archive_pending_recovery(f.db.clone(), archived.revision.clone()).await,
        Ok(ArchiveOutcome::NothingPending)
    );
    assert_eq!(
        delete_confirmed_recovery(
            f.db.clone(),
            archived.records[0].id.clone(),
            archived.revision
        )
        .await,
        Err(RuntimeError::Transaction(TransactionError::Recovery(
            RecoveryError::Unconfirmed
        )))
    );
    assert_eq!(f.probe.calls.load(Ordering::Relaxed), calls);
    assert!(!f.native_root.exists());
    assert_eq!(
        std::fs::read(parked.join("credentials.json")).unwrap(),
        f.fresh.to_bytes().unwrap()
    );
    assert_eq!(std::fs::read(root.join(PROFILE_FILE)).unwrap(), profiles);
    assert!(!root.join(JOURNAL_FILE).exists());
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_recovery_confirmation_and_recapture_require_native_admission() {
    let f = Fixture::new();
    f.interrupt_before_native().await;
    let archived = f.archive_pending().await;
    let catalog = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    let root = f.db.secret_session().root();
    let record = std::fs::read(root.join(RECOVERY_FILE)).unwrap();
    let profiles = std::fs::read(root.join(PROFILE_FILE)).unwrap();
    // Admission and root gates must win over an unavailable native credential file.
    std::fs::remove_file(f.native_root.join("credentials.json")).unwrap();
    for reason in [
        BlockedReason::UnsupportedBuild,
        BlockedReason::AppRunning,
        BlockedReason::ContextChanged,
    ] {
        {
            let mut observation = f.probe.observation.lock().unwrap();
            observation.install.artifact_sha256 = if reason == BlockedReason::UnsupportedBuild {
                [9; 32]
            } else {
                [7; 32]
            };
            observation.writers = if reason == BlockedReason::AppRunning {
                WriterState::Running
            } else {
                WriterState::Stopped
            };
            if reason == BlockedReason::ContextChanged {
                observation.root_identity[1] ^= 1;
            }
        }
        assert_eq!(
            confirm_archived_recovery(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                archived.records[0].id.clone(),
                archived.revision.clone()
            )
            .await,
            Err(RuntimeError::Blocked(reason))
        );
        assert_eq!(
            capture_and_confirm_recovery(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                archived.records[0].id.clone(),
                archived.revision.clone(),
                catalog.revision.clone()
            )
            .await,
            Err(RuntimeError::Blocked(reason))
        );
    }
    assert_eq!(std::fs::read(root.join(RECOVERY_FILE)).unwrap(), record);
    assert_eq!(std::fs::read(root.join(PROFILE_FILE)).unwrap(), profiles);
    assert!(!f.native_root.join("credentials.json").exists());
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_explicit_recapture_saves_new_native_login_before_resolving_record() {
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let f = Fixture::for_family(family);
        f.interrupt_before_native().await;
        let archived = f.archive_pending().await;
        let payload = f.recovery_payload();
        let fresh_c = f.native_login(family, "c", "new-login-c");
        let catalog = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        let id = archived.records[0].id.clone();
        let recovery_revision = archived.revision.clone();
        assert_eq!(
            confirm_archived_recovery(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                id.clone(),
                recovery_revision.clone()
            )
            .await,
            Err(RuntimeError::Transaction(TransactionError::Recovery(
                RecoveryError::ConfirmationMismatch
            )))
        );
        let profiles = std::fs::read(f.db.secret_session().root().join(PROFILE_FILE)).unwrap();
        for (recovery_revision, catalog_revision, expected) in [
            (
                "stale recovery revision".to_owned(),
                catalog.revision.clone(),
                TransactionError::RecoveryChanged,
            ),
            (
                recovery_revision.clone(),
                "stale catalog revision".to_owned(),
                TransactionError::CatalogChanged,
            ),
        ] {
            assert_eq!(
                capture_and_confirm_recovery(
                    f.db.clone(),
                    f.probe.clone(),
                    f.contracts.clone(),
                    id.clone(),
                    recovery_revision,
                    catalog_revision
                )
                .await,
                Err(RuntimeError::Transaction(expected))
            );
            assert_eq!(
                std::fs::read(f.db.secret_session().root().join(PROFILE_FILE)).unwrap(),
                profiles
            );
        }
        assert_eq!(
            capture_and_confirm_recovery(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                id,
                recovery_revision,
                catalog.revision
            )
            .await,
            Ok(CaptureOutcome::Saved)
        );
        let status = recovery_status(f.db.clone()).await.unwrap();
        assert!(!status.pending && !status.native_unconfirmed);
        assert_eq!(status.records[0].disposition, "explicit-capture");
        assert_eq!(f.recovery_payload(), payload);
        assert!(f.current() == fresh_c);
        let catalog = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
            .await
            .unwrap();
        assert_eq!(catalog.profiles.len(), 3);
        assert_eq!(
            switch_saved_account(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                f.b.opaque_id(),
                catalog.revision
            )
            .await,
            Ok(SwitchOutcome::Switched)
        );
        let status = recovery_status(f.db.clone()).await.unwrap();
        assert_eq!(status.records.len(), 2);
        assert!(status
            .records
            .iter()
            .any(|record| !record.latest_completed && record.disposition == "explicit-capture"));
    }
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_vault_delete_requires_current_selected_confirmed_record_without_native_access() {
    let f = Fixture::new();
    f.interrupt_before_native().await;
    let archived = f.archive_pending().await;
    let archive_id = archived.records[0].id.clone();
    assert_eq!(
        confirm_archived_recovery(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            archive_id.clone(),
            archived.revision.clone()
        )
        .await,
        Ok(())
    );
    assert_eq!(
        switch_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.b.clone()
        )
        .await,
        Ok(SwitchOutcome::Switched)
    );
    let status = recovery_status(f.db.clone()).await.unwrap();
    assert_eq!(status.records.len(), 2);
    let latest = status
        .records
        .iter()
        .find(|record| record.latest_completed)
        .unwrap()
        .id
        .clone();
    let root = f.db.secret_session().root();
    let recovery = std::fs::read(root.join(RECOVERY_FILE)).unwrap();
    let native = std::fs::read(f.native_root.join("credentials.json")).unwrap();
    let parked = f.native_root.with_file_name("unavailable-native-root");
    std::fs::rename(&f.native_root, &parked).unwrap();
    f.probe.observation.lock().unwrap().install.artifact_sha256 = [9; 32];
    let calls = f.probe.calls.load(Ordering::Relaxed);
    assert_eq!(
        delete_confirmed_recovery(f.db.clone(), archive_id.clone(), archived.revision).await,
        Err(RuntimeError::Transaction(TransactionError::RecoveryChanged))
    );
    assert_eq!(std::fs::read(root.join(RECOVERY_FILE)).unwrap(), recovery);
    assert_eq!(
        delete_confirmed_recovery(f.db.clone(), archive_id, status.revision).await,
        Ok(())
    );
    let remaining = recovery_status(f.db.clone()).await.unwrap();
    assert_eq!(remaining.records.len(), 1);
    assert_eq!(remaining.records[0].id, latest);
    {
        let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
        assert!(matches!(
            crate::secrets::reset::reset(root, "unused revision", "synthetic reset password"),
            Err(crate::error::AppError::Config(code)) if code == "secret.zcode_recovery_required"
        ));
    }
    assert_eq!(
        delete_confirmed_recovery(f.db.clone(), latest, remaining.revision).await,
        Ok(())
    );
    let empty = recovery_status(f.db.clone()).await.unwrap();
    assert!(empty.records.is_empty() && !empty.native_unconfirmed && !empty.pending);
    assert!(!root.join(RECOVERY_FILE).exists());
    assert_eq!(f.probe.calls.load(Ordering::Relaxed), calls);
    assert!(!f.native_root.exists());
    assert_eq!(
        std::fs::read(parked.join("credentials.json")).unwrap(),
        native
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn runtime_vault_recovery_cancellation_retains_owner_while_session_is_locked() {
    let f = Fixture::new();
    let writer_db = f.db.clone();
    let (acquired_send, acquired) = std::sync::mpsc::channel();
    let (release, receive) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let _write = writer_db.secret_session().write().unwrap();
        acquired_send.send(()).unwrap();
        receive.recv().unwrap();
    });
    acquired
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    // A different operation may own the process-global mutex while this request
    // is only queued. Its ownership must not satisfy target-worker readiness.
    let unrelated_owner = crate::services::sync_protocol::sync_mutex().lock().await;
    let db = f.db.clone();
    let (queued_send, queued) = tokio::sync::oneshot::channel();
    let owner_entered = Arc::new(tokio::sync::Notify::new());
    let operation = tokio::spawn(OWNER_ENTERED.scope(owner_entered.clone(), async move {
        let mut recovery = Box::pin(recovery_status(db));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(recovery.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        queued_send.send(()).unwrap();
        recovery.await
    }));
    queued.await.unwrap();
    let mut entered = Box::pin(owner_entered.notified());
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(entered.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(unrelated_owner);
    let ready = tokio::time::timeout(std::time::Duration::from_secs(5), entered).await;
    operation.abort();
    let caller_cancelled = matches!(operation.await, Err(error) if error.is_cancelled());
    let owner_retained = crate::services::sync_protocol::sync_mutex()
        .try_lock()
        .is_err();
    release.send(()).unwrap();
    writer.join().unwrap();
    assert!(ready.is_ok(), "the target blocking worker must enter");
    assert!(caller_cancelled);
    assert!(
        owner_retained,
        "caller cancellation must retain the entered blocking worker's owner"
    );
    let _sync = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::services::sync_protocol::sync_mutex().lock(),
    )
    .await
    .unwrap();
    assert_eq!(f.probe.calls.load(Ordering::Relaxed), 0);
    assert!(f.current() == f.fresh);
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_capture_review_is_masked_cancelable_and_one_use_even_when_kept() {
    let f = Fixture::new();
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    let root = f.db.secret_session().root();
    let vault_before = std::fs::read(root.join(PROFILE_FILE)).unwrap();
    let native_before = std::fs::read(f.native_root.join("credentials.json")).unwrap();
    let preview = preview_capture_account(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        status.revision.clone(),
    )
    .await
    .unwrap();
    let safe = serde_json::to_string(&preview).unwrap();
    assert!(
        !safe.contains("nativeRevision") && !safe.contains("token") && !safe.contains("credential")
    );
    assert!(preview.duplicate);
    assert_eq!(
        capture_reviewed_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            status.revision.clone(),
            preview.preview_id.clone(),
            false
        )
        .await,
        Ok(CaptureCommitOutcome::Kept)
    );
    assert_eq!(
        capture_reviewed_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            status.revision.clone(),
            preview.preview_id,
            true
        )
        .await,
        Err(RuntimeError::Transaction(TransactionError::SourceChanged))
    );
    let preview = preview_capture_account(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        status.revision.clone(),
    )
    .await
    .unwrap();
    cancel_capture_preview(f.db.clone(), preview.preview_id.clone())
        .await
        .unwrap();
    assert_eq!(
        capture_reviewed_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            status.revision,
            preview.preview_id,
            true
        )
        .await,
        Err(RuntimeError::Transaction(TransactionError::SourceChanged))
    );
    assert_eq!(
        std::fs::read(root.join(PROFILE_FILE)).unwrap(),
        vault_before
    );
    assert_eq!(
        std::fs::read(f.native_root.join("credentials.json")).unwrap(),
        native_before
    );
}

#[tokio::test]
#[serial_test::serial]
async fn runtime_capture_review_failed_admission_consumes_before_replay() {
    let f = Fixture::new();
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    let preview = preview_capture_account(
        f.db.clone(),
        f.probe.clone(),
        f.contracts.clone(),
        status.revision.clone(),
    )
    .await
    .unwrap();
    f.probe.observation.lock().unwrap().writers = WriterState::Running;
    assert_eq!(
        capture_reviewed_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            status.revision.clone(),
            preview.preview_id.clone(),
            false
        )
        .await,
        Err(RuntimeError::Blocked(BlockedReason::AppRunning))
    );
    f.probe.observation.lock().unwrap().writers = WriterState::Stopped;
    assert_eq!(
        capture_reviewed_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            status.revision,
            preview.preview_id,
            false
        )
        .await,
        Err(RuntimeError::Transaction(TransactionError::SourceChanged))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn runtime_coordinated_cancelled_caller_keeps_queryable_original_result() {
    let f = Fixture::new();
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    f.probe.observation.lock().unwrap().writers = WriterState::Running;
    let probe = Arc::new(CoordinatedProbe {
        inner: f.probe.clone(),
        quits: AtomicUsize::new(0),
        restarts: AtomicUsize::new(0),
        remains_running: false,
        restart_fails: false,
    });
    let (release, receive) = std::sync::mpsc::channel();
    let at = f.probe.calls.load(Ordering::Relaxed) + 1;
    *f.probe.pause.lock().unwrap() = Some((at, receive));
    let id = uuid::Uuid::new_v4().to_string();
    let task = tokio::spawn(switch_account_request(
        f.db.clone(),
        probe.clone(),
        f.contracts.clone(),
        f.b.opaque_id(),
        status.revision,
        id.clone(),
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.probe.entered.notified(),
    )
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(crate::services::sync_protocol::sync_mutex()
        .try_lock()
        .is_err());
    release.send(()).unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        operation_status(f.db.clone(), probe.clone(), f.contracts.clone(), id),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(
        result.phase,
        super::super::operation_log::Phase::RestartVerified
    );
    assert_eq!(probe.quits.load(Ordering::Relaxed), 1);
    assert_eq!(probe.restarts.load(Ordering::Relaxed), 1);
}
#[tokio::test]
#[serial_test::serial]
async fn runtime_coordinated_missing_source_does_not_exhaust_unknown_capacity_and_old_id_cannot_replay(
) {
    let f = Fixture::new();
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    let before = f.current().to_bytes().unwrap();
    let observed = f.probe.observation.lock().unwrap().clone();
    let context = VerifiedContext::assess(observed.clone(), &f.contracts).unwrap();
    let secret = zeroize::Zeroizing::new(format!(
        "zcode-credential-fallback:darwin:{}:synthetic-user",
        observed.home
    ));
    let unknown = native_document_with_context(
        context.context_id(),
        &secret,
        OAuthFamily::Zai,
        "not-saved",
        "synthetic",
    );
    write_durable(
        &f.native_root.join("credentials.json"),
        &unknown.to_bytes().unwrap(),
    )
    .unwrap();
    let first = uuid::Uuid::new_v4().to_string();
    for index in 0..17 {
        let id = if index == 0 {
            first.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        assert_eq!(
            switch_account_request(
                f.db.clone(),
                f.probe.clone(),
                f.contracts.clone(),
                f.b.opaque_id(),
                status.revision.clone(),
                id.clone()
            )
            .await,
            Err(RuntimeError::Transaction(
                TransactionError::MissingSavedSource
            ))
        );
        assert_eq!(
            operation_status(f.db.clone(), f.probe.clone(), f.contracts.clone(), id)
                .await
                .unwrap()
                .unwrap()
                .phase,
            super::super::operation_log::Phase::Failed
        );
    }
    write_durable(&f.native_root.join("credentials.json"), &before).unwrap();
    let probe = Arc::new(CoordinatedProbe {
        inner: f.probe.clone(),
        quits: AtomicUsize::new(0),
        restarts: AtomicUsize::new(0),
        remains_running: false,
        restart_fails: false,
    });
    assert_eq!(
        switch_account_request(
            f.db.clone(),
            probe.clone(),
            f.contracts.clone(),
            f.b.opaque_id(),
            status.revision.clone(),
            first
        )
        .await,
        Err(RuntimeError::Transaction(
            TransactionError::OperationAlreadyKnown
        ))
    );
    assert_eq!(probe.quits.load(Ordering::Relaxed), 0);
    assert_eq!(probe.restarts.load(Ordering::Relaxed), 0);
    assert_eq!(f.current().to_bytes().unwrap(), before);
    assert_eq!(
        switch_account_request(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.b.opaque_id(),
            status.revision,
            uuid::Uuid::new_v4().to_string()
        )
        .await,
        Ok(SwitchOutcome::Switched)
    );
}
