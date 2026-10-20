//! Synthetic integration tests against the original product vault owners.
//! No network or native TRAE installation is used. Run with --test-threads=1.
use super::{import, inference::Lease, store};
use crate::{
    database::{vault, Database},
    secrets::{
        export, files,
        key_store::{KeyStore, KeyStoreError},
        owned_file::OwnedFile,
        reset, rewrap,
        session::SecretSession,
        testing::{MemoryKeyStore, TestHome},
        transition, VaultContext,
    },
};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;

const FILE: &str = "trae_accounts.json";
const PASSWORD: &str = "synthetic TRAE lifecycle recovery password";

/// Only the external key-store failure is synthetic; the original rotate /
/// metadata / inventory / durable publication / recovery implementations run.
#[derive(Default)]
struct FailOneRemoval {
    inner: MemoryKeyStore,
    fail: AtomicBool,
}
impl KeyStore for FailOneRemoval {
    fn load(&self, vault: &str, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        self.inner.load(vault, key)
    }
    fn save(&self, vault: &str, key: &str, bytes: &[u8]) -> Result<(), KeyStoreError> {
        self.inner.save(vault, key, bytes)
    }
    fn remove(&self, vault: &str, key: &str) -> Result<(), KeyStoreError> {
        if self.fail.swap(false, Ordering::AcqRel) {
            return Err(KeyStoreError::Unavailable);
        }
        self.inner.remove(vault, key)
    }
}
struct Fixture {
    db: Database,
    keys: FailOneRemoval,
    root: PathBuf,
    id: String,
    _home: TestHome,
    _owner: tokio::sync::OwnedMutexGuard<()>,
}
impl Fixture {
    fn new(pending: bool) -> Self {
        let owner = crate::rt::block_on(store::fixture_guard());
        let home = TestHome::new().unwrap();
        let root = home.path().join(crate::APP_DIR_NAME);
        assert!(
            root.starts_with(home.path()),
            "synthetic root must stay isolated"
        );
        let keys = FailOneRemoval::default();
        let session = SecretSession::open(&root, &keys, None).unwrap();
        let conn = vault::prepare(
            &root.join(crate::config::DB_FILE_NAME),
            &session.read().unwrap(),
        )
        .unwrap();
        let db = Database::from_connection(conn, session.clone());
        session.complete_migration().unwrap();
        let id = "a".repeat(64);
        let mut account = import::synthetic_material().account;
        account.realm = store::Realm::Sg;
        account.device.auth_host = store::Realm::Sg.auth().into();
        account.id = id.clone();
        account.refresh_pending = pending;
        crate::rt::block_on(store::access(&session, None, |d, _| {
            d.accounts.push(account);
            Ok(((), true))
        }))
        .unwrap();
        Self {
            db,
            keys,
            root,
            id,
            _home: home,
            _owner: owner,
        }
    }
    fn file(&self) -> OwnedFile {
        OwnedFile::registered(FILE).unwrap()
    }
    fn plaintext(&self) -> Zeroizing<Vec<u8>> {
        self.file().read(&self.db.secrets).unwrap()
    }
    fn serialized_hash(&self) -> String {
        hex::encode(Sha256::digest(self.plaintext().as_slice()))
    }
    fn with_sync<T>(&self, f: impl FnOnce() -> T) -> T {
        let _sync = crate::rt::block_on(crate::services::sync_protocol::sync_mutex().lock());
        f()
    }
    fn restart_child(&self, expected_hash: &str, expected_pending: bool) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "trae_accounts::lifecycle_tests::restart_fixture_child",
                "--ignored",
                "--test-threads=1",
            ])
            .env("TRAE_LIFECYCLE_SYNTHETIC_ROOT", &self.root)
            .env("TRAE_LIFECYCLE_SYNTHETIC_HASH", expected_hash)
            .env(
                "TRAE_LIFECYCLE_SYNTHETIC_PENDING",
                if expected_pending { "1" } else { "0" },
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fresh synthetic child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[serial_test::serial]
fn registered_inventory_covers_current_and_recovery_without_plaintext_fallback() {
    let f = Fixture::new(false);
    let live = std::fs::read(f.root.join(FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&live).contains("synthetic-refresh"));
    let recovery = f.root.join("backups/vault-recovery").join(FILE);
    crate::config_file_io::write_durable(&recovery, &live).unwrap();
    let vault = f.db.secrets.read().unwrap();
    let plans = files::stage_owned_files_with_vault(&f.root, &vault, false).unwrap();
    let selected: Vec<_> = plans
        .iter()
        .filter(|p| p.file.relative_path() == std::path::Path::new(FILE))
        .collect();
    assert_eq!(selected.len(), 2);
    for plan in selected {
        assert_eq!(plan.ciphertext, live);
        assert_eq!(
            plan.file
                .decode(&vault, &plan.ciphertext)
                .unwrap()
                .as_slice(),
            f.file().decode(&vault, &live).unwrap().as_slice()
        );
    }
    let plain = f.file().decode(&vault, &live).unwrap();
    crate::config_file_io::write_durable(&recovery, &plain).unwrap();
    assert!(files::stage_owned_files_with_vault(&f.root, &vault, true).is_err());
}

#[test]
#[serial_test::serial]
fn password_rewrap_preserves_account_and_reopens_in_a_fresh_process() {
    let f = Fixture::new(false);
    let expected = f.serialized_hash();
    let before = std::fs::read(f.root.join(FILE)).unwrap();
    let old = f.db.secrets.read().unwrap().metadata().clone();
    let fence = crate::rt::block_on(Lease::acquire(
        f.db.secrets.clone(),
        &f.id,
        "synthetic-lifecycle",
    ))
    .unwrap()
    .fence();
    f.with_sync(|| rewrap::change_password(&f.db, &f.keys, PASSWORD, false))
        .unwrap();
    let next = f.db.secrets.read().unwrap().metadata().clone();
    assert_eq!(next.key_id, old.key_id);
    assert_eq!(next.vault_id, old.vault_id);
    assert!(next.revision > old.revision);
    assert_eq!(std::fs::read(f.root.join(FILE)).unwrap(), before);
    assert!(crate::rt::block_on(fence.check()).is_err());
    assert!(SecretSession::open_existing(&f.root, &MemoryKeyStore::default(), None).is_err());
    assert!(SecretSession::open_existing(
        &f.root,
        &MemoryKeyStore::default(),
        Some("wrong synthetic password")
    )
    .is_err());
    f.restart_child(&expected, false);
    assert_eq!(f.serialized_hash(), expected);
}

#[test]
#[serial_test::serial]
fn interrupted_original_rotation_recovers_in_fresh_process_without_enabling_pending() {
    let f = Fixture::new(true);
    let expected = f.serialized_hash();
    let old = f.db.secrets.read().unwrap().clone();
    let live = std::fs::read(f.root.join(FILE)).unwrap();
    let recovery = f.root.join("backups/vault-recovery").join(FILE);
    crate::config_file_io::write_durable(&recovery, &live).unwrap();
    f.keys.fail.store(true, Ordering::Release);
    assert!(f
        .with_sync(|| transition::rotate(&f.db, &f.keys, PASSWORD, false))
        .is_err());
    assert!(
        !f.keys.fail.load(Ordering::Acquire),
        "must reach the real late key-store removal boundary"
    );
    assert!(f.root.join(transition::INTENT).is_file());
    assert!(f.db.secrets.read().is_err());
    assert!(crate::rt::block_on(store::snapshot(&f.db.secrets, &f.id)).is_err());
    f.restart_child(&expected, true);
    assert!(!f.root.join(transition::INTENT).exists());
    let reopened =
        SecretSession::open_existing(&f.root, &MemoryKeyStore::default(), Some(PASSWORD)).unwrap();
    let next = reopened.read().unwrap();
    assert_eq!(next.metadata().vault_id, old.metadata().vault_id);
    assert_ne!(next.metadata().key_id, old.metadata().key_id);
    for path in [f.root.join(FILE), recovery] {
        let bytes = std::fs::read(path).unwrap();
        assert!(f.file().decode(&old, &bytes).is_err());
        assert_eq!(
            hex::encode(Sha256::digest(
                f.file().decode(&next, &bytes).unwrap().as_slice()
            )),
            expected
        );
    }
    drop(next);
    assert!(crate::rt::block_on(store::snapshot(&reopened, &f.id)).is_err());
    assert_eq!(
        crate::rt::block_on(store::lookup(&reopened, &f.id, store::Realm::Sg))
            .unwrap()
            .unwrap()
            .status,
        store::AccountStatus::RefreshPending
    );
}

#[test]
#[serial_test::serial]
fn explicit_reset_clears_live_account_and_retains_only_encrypted_recovery_archive() {
    let f = Fixture::new(true);
    let expected = f.serialized_hash();
    let old = f.db.secrets.read().unwrap().clone();
    let file = f.file();
    // Key-loss reset is an offline owner. Close the original database before
    // renaming its tree, matching the product path on Windows as well as Unix.
    let Fixture {
        db,
        keys: _keys,
        root,
        id,
        _home,
        _owner,
    } = f;
    drop(db);
    let sync = crate::rt::block_on(crate::services::sync_protocol::sync_mutex().lock());
    let preview = reset::preview(&root).unwrap();
    let archive_path = reset::reset(&root, &preview.fingerprint, PASSWORD).unwrap();
    drop(sync);
    assert!(!root.join(FILE).exists());
    let fresh =
        SecretSession::open_existing(&root, &MemoryKeyStore::default(), Some(PASSWORD)).unwrap();
    assert!(crate::rt::block_on(store::list(&fresh)).unwrap().is_empty());
    assert!(crate::rt::block_on(store::snapshot(&fresh, &id)).is_err());
    let bytes = std::fs::read(archive_path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-refresh"));
    let archive: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let metadata = serde_json::from_value(archive["metadata"].clone()).unwrap();
    let next = VaultContext::from_password(metadata, PASSWORD).unwrap();
    let plain = next
        .open(
            &["local", "reset-archive", archive["id"].as_str().unwrap()],
            archive["content"].as_str().unwrap(),
        )
        .unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(plain.as_slice())).unwrap();
    let mut sealed = Zeroizing::new(Vec::new());
    zip.by_name(&format!("data/{FILE}"))
        .unwrap()
        .read_to_end(&mut sealed)
        .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("synthetic-refresh"));
    assert_eq!(
        hex::encode(Sha256::digest(
            file.decode(&old, &sealed).unwrap().as_slice()
        )),
        expected
    );
}

#[test]
#[serial_test::serial]
fn current_plaintext_export_does_not_expand_into_private_trae_accounts() {
    let f = Fixture::new(false);
    let before = std::fs::read(f.root.join(FILE)).unwrap();
    let exported = f
        .with_sync(|| export::build_plaintext_export(&f.db.secrets))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert!(value["files"].get(FILE).is_none());
    for sensitive in [
        "synthetic-user",
        "synthetic-refresh",
        "synthetic-access",
        "PRIVATE KEY",
    ] {
        assert!(!exported.contains(sensitive));
    }
    assert_eq!(std::fs::read(f.root.join(FILE)).unwrap(), before);
}

#[test]
#[ignore = "Only the parent integration tests launch this isolated synthetic process"]
fn restart_fixture_child() {
    let root = PathBuf::from(
        std::env::var_os("TRAE_LIFECYCLE_SYNTHETIC_ROOT").expect("parent-only fixture"),
    );
    let home =
        PathBuf::from(std::env::var_os("CC_SWITCH_TEST_HOME").expect("isolated home required"));
    let temporary = std::env::temp_dir().canonicalize().unwrap();
    assert!(home.canonicalize().unwrap().starts_with(temporary));
    assert!(root.starts_with(&home));
    assert_eq!(root.file_name().unwrap(), crate::APP_DIR_NAME);
    let expected = std::env::var("TRAE_LIFECYCLE_SYNTHETIC_HASH").unwrap();
    let pending = std::env::var("TRAE_LIFECYCLE_SYNTHETIC_PENDING").unwrap() == "1";
    let _sync = crate::rt::block_on(crate::services::sync_protocol::sync_mutex().lock());
    let session =
        SecretSession::open_existing(&root, &MemoryKeyStore::default(), Some(PASSWORD)).unwrap();
    let plain = OwnedFile::registered(FILE).unwrap().read(&session).unwrap();
    assert_eq!(hex::encode(Sha256::digest(plain.as_slice())), expected);
    drop(_sync);
    let views = crate::rt::block_on(store::list(&session)).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].realm, store::Realm::Sg);
    assert_eq!(
        views[0].status,
        if pending {
            store::AccountStatus::RefreshPending
        } else {
            store::AccountStatus::Stored
        }
    );
    assert_eq!(
        crate::rt::block_on(store::snapshot(&session, &views[0].id)).is_err(),
        pending
    );
}
