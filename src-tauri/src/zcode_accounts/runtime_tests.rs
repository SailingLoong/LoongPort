use super::super::admission::{
    BuildFingerprint, KeyContextChoice, Platform, VerifiedContext, WriterState,
};
use super::super::checkpoint::{ProfileCatalog, PROFILE_FILE};
use super::super::core::{CredentialDocument, OAuthFamily};
use super::super::native::tests::native_document_with_context;
use super::*;
use crate::config_file_io::{ensure_private_directory, write_durable};
use crate::secrets::{session::SecretSession, testing::MemoryKeyStore};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct FakeProbe {
    observation: Mutex<ContextObservation>,
    calls: AtomicUsize,
    pause: Mutex<Option<(usize, std::sync::mpsc::Receiver<()>)>>,
    fail_at: AtomicUsize,
    entered: tokio::sync::Notify,
    origin: Mutex<Option<JournalOrigin>>,
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
    fn journal_origin(&self) -> Option<JournalOrigin> {
        *self.origin.lock().unwrap()
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
            standard_desktop_launch: true,
            key_choice: KeyContextChoice::ExplicitStandard,
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
            origin: Mutex::new(Some(JournalOrigin::Live)),
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
async fn runtime_rejects_blocked_vault_and_untrusted_recovery_source() {
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
    f.db.secrets.set_blocked(false);
    *f.probe.origin.lock().unwrap() = None;
    assert_eq!(
        recover_account(f.db.clone(), f.probe.clone(), f.contracts.clone()).await,
        Err(RuntimeError::OriginUnverified)
    );
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
async fn runtime_pending_transaction_blocks_real_vault_maintenance_then_recovers() {
    let f = Fixture::new();
    // Initial admission1, constructor2, lock3/4, prepared5, profile6,
    // captured7; refuse the native publication at8.
    f.probe.fail_at.store(8, Ordering::Relaxed);
    assert_eq!(
        switch_account(
            f.db.clone(),
            f.probe.clone(),
            f.contracts.clone(),
            f.b.clone()
        )
        .await,
        Err(RuntimeError::Transaction(TransactionError::Admission(
            BlockedReason::AppRunning
        )))
    );
    let root = f.db.secret_session().root();
    assert!(root.join(crate::secrets::owned_file::JOURNAL_FILE).exists());
    assert!(f.current() == f.fresh);
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
    assert!(f.current() == f.fresh);
    f.probe.fail_at.store(0, Ordering::Relaxed);
    assert_eq!(
        recover_account(f.db.clone(), f.probe.clone(), f.contracts.clone()).await,
        Ok(SwitchOutcome::Recovered)
    );
    {
        let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
        crate::secrets::rewrap::change_password(
            &f.db,
            &f._store,
            "synthetic maintenance password",
            false,
        )
        .unwrap();
    }
    let status = account_status(f.db.clone(), f.probe.clone(), f.contracts.clone())
        .await
        .unwrap();
    assert!(!status.pending);
    assert_eq!(status.profiles.len(), 2);
}
