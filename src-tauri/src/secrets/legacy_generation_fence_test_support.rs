//! Synthetic fixtures shared only by the legacy-writer fence tests.
use crate::{
    database::Database,
    error::AppError,
    secrets::{
        key_store::{KeyStore, KeyStoreError},
        protection::{ProtectionRecord, ProtectionStorage},
        storage::PlaintextIdentity,
        testing::{MemoryKeyStore, TestHome},
        VaultContext,
    },
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::SystemTime,
};
use zeroize::Zeroizing;
#[derive(Default)]
pub(crate) struct CountingStore {
    calls: AtomicUsize,
    inner: MemoryKeyStore,
}
impl CountingStore {
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub(crate) fn reset(&self) {
        self.calls.store(0, Ordering::SeqCst);
    }
}
impl KeyStore for CountingStore {
    fn load(&self, v: &str, k: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.load(v, k)
    }
    fn save(&self, v: &str, k: &str, b: &[u8]) -> Result<(), KeyStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.save(v, k, b)
    }
    fn remove(&self, v: &str, k: &str) -> Result<(), KeyStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.remove(v, k)
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Scope {
    Encrypted,
    Plaintext,
    Malformed,
}
pub(crate) const SCOPES: [Scope; 3] = [Scope::Encrypted, Scope::Plaintext, Scope::Malformed];
pub(crate) const PASSWORD: &str = "synthetic legacy protection password";
pub(crate) fn write_record(root: &Path, scope: Scope, current: &VaultContext) {
    std::fs::create_dir_all(root).unwrap();
    let storage = match scope {
        Scope::Encrypted => ProtectionStorage::Encrypted {
            vault_id: current.metadata().vault_id.clone(),
            key_id: current.metadata().key_id.clone(),
        },
        Scope::Plaintext => ProtectionStorage::Plaintext {
            identity: PlaintextIdentity {
                format_version: 1,
                store_id: current.metadata().vault_id.clone(),
                generation_id: current.metadata().key_id.clone(),
                revision: current.metadata().revision,
            },
        },
        Scope::Malformed => {
            std::fs::write(root.join("protection.json"), b"{\"formatVersion\":").unwrap();
            return;
        }
    };
    let record = ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: true,
        storage,
    };
    record.validate().unwrap();
    std::fs::write(
        root.join("protection.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}
pub(crate) fn assert_rejected<T>(result: Result<T, AppError>, scope: Scope) {
    let error = result
        .err()
        .expect("protected scope unexpectedly entered a legacy writer");
    let expected = match scope {
        Scope::Malformed => "protection.invalid_record",
        _ => "protection.generation_owner_required",
    };
    assert!(
        matches!(error,AppError::Config(ref code) if code==expected),
        "unexpected fence error: {error}"
    );
}
type Tree = BTreeMap<PathBuf, (bool, u64, String, Option<SystemTime>)>;
pub(crate) fn tree(root: &Path) -> Tree {
    fn walk(base: &Path, path: &Path, out: &mut Tree) {
        let meta = std::fs::symlink_metadata(path).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "synthetic fixture unexpectedly contains a link"
        );
        let bytes = if meta.is_file() {
            std::fs::read(path).unwrap()
        } else {
            Vec::new()
        };
        out.insert(
            path.strip_prefix(base).unwrap().to_owned(),
            (
                meta.is_dir(),
                bytes.len() as u64,
                crate::services::sync_protocol::sha256_hex(&bytes),
                meta.modified().ok(),
            ),
        );
        if meta.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                walk(base, &entry.unwrap().path(), out);
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}
pub(crate) struct Fixture {
    pub(crate) db: Database,
    pub(crate) store: CountingStore,
    pub(crate) home: TestHome,
}
impl Fixture {
    pub(crate) fn new() -> Self {
        let home = TestHome::new().unwrap();
        let db = crate::secrets::testing::initialize_source17_database().unwrap();
        db.set_setting("global_proxy_url", "synthetic-fence-value")
            .unwrap();
        Self {
            db,
            store: CountingStore::default(),
            home,
        }
    }
    pub(crate) fn with_user_text() -> Self {
        let home = TestHome::new().unwrap();
        let root = crate::config::get_app_config_dir();
        let context = VaultContext::create_generation_with_user_text(
            uuid::Uuid::new_v4().to_string(),
            1,
            "synthetic exact text",
        )
        .unwrap();
        crate::config::ensure_private_directory(&root).unwrap();
        crate::secrets::session::write_metadata(
            &root,
            &crate::secrets::session::completed_metadata(&context, false).unwrap(),
        )
        .unwrap();
        let conn =
            crate::database::vault::prepare(&root.join(crate::config::DB_FILE_NAME), &context)
                .unwrap();
        let session = crate::secrets::session::SecretSession::from_context(root, context);
        let db = Database::from_connection(conn, session);
        db.set_setting("global_proxy_url", "synthetic-fence-value")
            .unwrap();
        Self {
            db,
            store: CountingStore::default(),
            home,
        }
    }
    pub(crate) fn root(&self) -> &Path {
        self.db.secret_session().root()
    }
    pub(crate) fn current(&self) -> VaultContext {
        self.db
            .secret_session()
            .read()
            .unwrap()
            .encrypted()
            .unwrap()
            .clone()
    }
    pub(crate) fn record(&self, scope: Scope) {
        write_record(self.root(), scope, &self.current());
    }
}
