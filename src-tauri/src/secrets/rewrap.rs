//! Password changes preserve the data key. A durable intent repairs the SQLite /
//! metadata / operating-system-store commit boundary after interruption.

use super::{
    key_store::{save_verified, KeyStore},
    session::{read_metadata, write_durable, write_metadata, LocalVault},
    VaultContext,
};
use crate::{
    database::{lock_conn, vault, Database},
    error::AppError,
};
use rusqlite::Connection;
use std::path::Path;

const INTENT: &str = ".vault-rewrap";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordIntent {
    candidate: super::VaultMetadata,
    body: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordChange {
    previous_automatic_unlock: bool,
    next: LocalVault,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProtectionStatus {
    automatic_unlock: bool,
    password_configured: bool,
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) fn get_secret_protection(
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<ProtectionStatus, String> {
    let _vault = state.db.secrets.read().map_err(super::error::public_code)?;
    let saved = read_metadata(state.db.secrets.root()).map_err(super::error::public_code)?;
    Ok(ProtectionStatus {
        automatic_unlock: saved.automatic_unlock,
        password_configured: saved.metadata.wrapped_key.is_some(),
    })
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn set_secret_password(
    state: tauri::State<'_, crate::store::AppState>,
    password: String,
    automatic_unlock: bool,
) -> Result<(), String> {
    let db = state.db.clone();
    let password = zeroize::Zeroizing::new(password);
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        change_password(
            &db,
            &super::key_store::SystemKeyStore,
            &password,
            automatic_unlock,
        )
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
    .map_err(super::error::public_code)
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn rotate_secret_key(
    state: tauri::State<'_, crate::store::AppState>,
    password: String,
    automatic_unlock: bool,
) -> Result<(), String> {
    let db = state.db.clone();
    let password = zeroize::Zeroizing::new(password);
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        super::transition::rotate(
            &db,
            &super::key_store::SystemKeyStore,
            &password,
            automatic_unlock,
        )
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
    .map_err(super::error::public_code)
}

/// Caller owns the shared sync operation mutex before entering this method.
pub(crate) fn change_password(
    db: &Database,
    store: &dyn KeyStore,
    password: &str,
    automatic_unlock: bool,
) -> Result<(), AppError> {
    let session = &db.secrets;
    let mut current = session.write()?;
    if session.root().join(".vault-transition").exists() || session.root().join(INTENT).exists() {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let next = current
        .with_password(password)
        .map_err(super::inventory::secret_error)?;
    let mut saved = read_metadata(session.root())?;
    let previous_automatic_unlock = saved.automatic_unlock;
    saved.metadata = next.metadata().clone();
    saved.automatic_unlock = automatic_unlock;
    let change = PasswordChange {
        previous_automatic_unlock,
        next: saved,
    };
    let bytes = serde_json::to_vec(&change)
        .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    let intent = current
        .seal(&["local", "password-transition"], &bytes)
        .map_err(super::inventory::secret_error)?;
    let conn = lock_conn!(db.conn);
    session.set_blocked(true);
    let intent = serde_json::to_vec(&PasswordIntent {
        candidate: next.metadata().clone(),
        body: intent,
    })
    .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    write_durable(&session.root().join(INTENT), &intent)?;
    finish(session.root(), &conn, &current, &next, &change, store)?;
    *current = next;
    session.set_blocked(false);
    Ok(())
}

fn finish(
    root: &Path,
    conn: &Connection,
    current: &VaultContext,
    next: &VaultContext,
    change: &PasswordChange,
    store: &dyn KeyStore,
) -> Result<(), AppError> {
    let saved = &change.next;
    let metadata = vault::stored_metadata(conn)?
        .ok_or_else(|| AppError::Config("secret.metadata_missing".into()))?;
    if metadata != *current.metadata() && metadata != *next.metadata() {
        return Err(AppError::Config("secret.identity_mismatch".into()));
    }
    if saved.automatic_unlock {
        save_verified(
            store,
            &saved.metadata.vault_id,
            &saved.metadata.key_id,
            &next.export_key(),
        )
        .map_err(|_| AppError::Config("secret.store_unavailable".into()))?;
    }
    vault::stamp(conn, next)?;
    write_metadata(root, saved)?;
    if change.previous_automatic_unlock && !saved.automatic_unlock {
        store
            .remove(&saved.metadata.vault_id, &saved.metadata.key_id)
            .map_err(|_| AppError::Config("secret.store_unavailable".into()))?;
        if store
            .load(&saved.metadata.vault_id, &saved.metadata.key_id)
            .map_err(|_| AppError::Config("secret.store_unavailable".into()))?
            .is_some()
        {
            return Err(AppError::Config("secret.key_removal_failed".into()));
        }
    }
    let path = root.join(INTENT);
    std::fs::remove_file(&path).map_err(|e| AppError::io(&path, e))?;
    #[cfg(unix)]
    std::fs::File::open(root)
        .and_then(|f| f.sync_all())
        .map_err(|e| AppError::io(root, e))?;
    Ok(())
}

pub(crate) fn recover(
    root: &Path,
    current: &mut VaultContext,
    store: &dyn KeyStore,
) -> Result<(), AppError> {
    let path = root.join(INTENT);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io(&path, e)),
    };
    if bytes.len() > 128 * 1024 {
        return Err(AppError::Config("secret.invalid_metadata".into()));
    }
    let intent: PasswordIntent = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    let plaintext = current
        .open(&["local", "password-transition"], &intent.body)
        .map_err(super::inventory::secret_error)?;
    let change: PasswordChange = serde_json::from_slice(&plaintext)
        .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    let saved = &change.next;
    if saved.metadata != intent.candidate
        || saved.metadata.vault_id != current.metadata().vault_id
        || saved.metadata.key_id != current.metadata().key_id
        || !(saved.metadata.revision == current.metadata().revision
            || saved.metadata.revision == current.metadata().revision.saturating_add(1))
    {
        return Err(AppError::Config("secret.identity_mismatch".into()));
    }
    let next = VaultContext::from_key(saved.metadata.clone(), current.export_key())
        .map_err(super::inventory::secret_error)?;
    let database = root.join(crate::config::DB_FILE_NAME);
    vault::preflight(&database)?;
    let conn = Connection::open(&database).map_err(|e| AppError::Database(e.to_string()))?;
    finish(root, &conn, current, &next, &change, store)?;
    *current = next;
    Ok(())
}

/// The candidate wrapper makes an interrupted password change recoverable using
/// the new password even before the active metadata file has been replaced.
pub(crate) fn recover_with_password(
    root: &Path,
    store: &dyn KeyStore,
    password: Option<&str>,
) -> Result<(), AppError> {
    let Some(password) = password else {
        return Ok(());
    };
    let path = root.join(INTENT);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io(&path, e)),
    };
    if bytes.len() > 128 * 1024 {
        return Err(AppError::Config("secret.invalid_metadata".into()));
    }
    let intent: PasswordIntent = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    let next = VaultContext::from_password(intent.candidate, password)
        .map_err(super::inventory::secret_error)?;
    let committed = read_metadata(root)?;
    // Possession of the candidate key must authenticate the committed vault;
    // an unrelated attacker-created wrapper cannot authorize replacement.
    let mut current = VaultContext::from_key(committed.metadata, next.export_key())
        .map_err(super::inventory::secret_error)?;
    recover(root, &mut current, store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{
        key_store::KeyStoreError, session::SecretSession, testing::MemoryKeyStore,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct FailingStore {
        inner: MemoryKeyStore,
        fail_remove: AtomicBool,
    }
    impl KeyStore for FailingStore {
        fn load(&self, v: &str, k: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
            self.inner.load(v, k)
        }
        fn save(&self, v: &str, k: &str, b: &[u8]) -> Result<(), KeyStoreError> {
            self.inner.save(v, k, b)
        }
        fn remove(&self, v: &str, k: &str) -> Result<(), KeyStoreError> {
            if self.fail_remove.swap(false, Ordering::SeqCst) {
                Err(KeyStoreError::Unavailable)
            } else {
                self.inner.remove(v, k)
            }
        }
    }

    fn fixture(root: &Path, store: &dyn KeyStore) -> Database {
        let secrets = SecretSession::open(root, store, None).unwrap();
        let conn = vault::prepare(
            &root.join(crate::config::DB_FILE_NAME),
            &secrets.read().unwrap(),
        )
        .unwrap();
        let db = Database::from_connection(conn, secrets);
        db.set_setting("global_proxy_url", "https://fixture.invalid")
            .unwrap();
        db
    }

    fn raw_secret(db: &Database) -> String {
        db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM settings WHERE key='global_proxy_url'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn password_only_mode_removes_auto_unlock_key_without_changing_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryKeyStore::default();
        let db = fixture(dir.path(), &store);
        let before = raw_secret(&db);
        let original = db.secrets.read().unwrap().metadata().clone();
        change_password(&db, &store, "example protection password", false).unwrap();
        assert_eq!(raw_secret(&db), before);
        assert!(store
            .load(&original.vault_id, &original.key_id)
            .unwrap()
            .is_none());
        assert!(SecretSession::open_existing(dir.path(), &store, None).is_err());
        assert!(
            SecretSession::open_existing(dir.path(), &store, Some("incorrect password")).is_err()
        );
        let reopened =
            SecretSession::open_existing(dir.path(), &store, Some("example protection password"))
                .unwrap();
        assert_eq!(reopened.read().unwrap().metadata().key_id, original.key_id);
        assert_eq!(
            db.get_setting("global_proxy_url").unwrap().as_deref(),
            Some("https://fixture.invalid")
        );
    }

    #[test]
    fn password_only_password_change_does_not_require_system_store() {
        struct UnavailableStore;
        impl KeyStore for UnavailableStore {
            fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
                Err(KeyStoreError::Unavailable)
            }
            fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeyStoreError> {
                Err(KeyStoreError::Unavailable)
            }
            fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
                Err(KeyStoreError::Unavailable)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryKeyStore::default();
        let db = fixture(dir.path(), &store);
        change_password(&db, &store, "original protection password", false).unwrap();
        change_password(
            &db,
            &UnavailableStore,
            "replacement protection password",
            false,
        )
        .unwrap();
        assert!(SecretSession::open_existing(
            dir.path(),
            &UnavailableStore,
            Some("replacement protection password")
        )
        .is_ok());
    }

    #[test]
    fn interrupted_key_removal_blocks_old_session_and_recovers_with_password() {
        let dir = tempfile::tempdir().unwrap();
        let store = FailingStore::default();
        let db = fixture(dir.path(), &store);
        store.fail_remove.store(true, Ordering::SeqCst);
        assert!(change_password(&db, &store, "example protection password", false).is_err());
        assert!(dir.path().join(INTENT).exists());
        assert!(db.secrets.read().is_err());
        assert!(db.set_current_provider("codex", "missing").is_err());
        assert!(db.delete_provider("codex", "missing").is_err());
        assert!(SecretSession::open_existing(dir.path(), &store, None).is_err());
        let resumed =
            SecretSession::open_existing(dir.path(), &store, Some("example protection password"))
                .unwrap();
        assert!(!dir.path().join(INTENT).exists());
        vault::check_identity(
            &Connection::open(dir.path().join(crate::config::DB_FILE_NAME)).unwrap(),
            &resumed.read().unwrap(),
        )
        .unwrap();
        assert!(db.secrets.read().is_err());
    }

    #[test]
    fn new_password_recovers_before_active_metadata_was_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryKeyStore::default();
        let db = fixture(dir.path(), &store);
        change_password(&db, &store, "original protection password", true).unwrap();
        let current = db.secrets.read().unwrap();
        let next = current
            .with_password("replacement protection password")
            .unwrap();
        let mut saved = read_metadata(dir.path()).unwrap();
        saved.metadata = next.metadata().clone();
        saved.automatic_unlock = false;
        let change = PasswordChange {
            previous_automatic_unlock: true,
            next: saved,
        };
        let body = current
            .seal(
                &["local", "password-transition"],
                &serde_json::to_vec(&change).unwrap(),
            )
            .unwrap();
        write_durable(
            &dir.path().join(INTENT),
            &serde_json::to_vec(&PasswordIntent {
                candidate: next.metadata().clone(),
                body,
            })
            .unwrap(),
        )
        .unwrap();
        drop(current);
        assert!(
            recover_with_password(dir.path(), &store, Some("incorrect protection password"))
                .is_err()
        );
        let resumed = SecretSession::open_existing(
            dir.path(),
            &store,
            Some("replacement protection password"),
        )
        .unwrap();
        assert!(!dir.path().join(INTENT).exists());
        vault::check_identity(
            &Connection::open(dir.path().join(crate::config::DB_FILE_NAME)).unwrap(),
            &resumed.read().unwrap(),
        )
        .unwrap();
        assert!(SecretSession::open_existing(
            dir.path(),
            &store,
            Some("original protection password")
        )
        .is_err());
    }

    #[test]
    fn invalid_password_does_not_start_a_transition() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryKeyStore::default();
        let db = fixture(dir.path(), &store);
        let metadata = std::fs::read(dir.path().join("vault.json")).unwrap();
        assert!(change_password(&db, &store, "short", false).is_err());
        assert!(!dir.path().join(INTENT).exists());
        assert_eq!(
            std::fs::read(dir.path().join("vault.json")).unwrap(),
            metadata
        );
        assert!(db.secrets.read().is_ok());
    }
}
