//! Database migration is owned by startup, before any business adapter is exposed.

use super::Database;
use crate::{error::AppError, secrets::VaultContext};
use rusqlite::Connection;
use std::path::Path;

fn db_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

/// Version checks must precede both key creation and any SQLite write.
pub(crate) fn preflight(path: &Path) -> Result<(), AppError> {
    if !path.exists() {
        return Ok(());
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(db_error)?;
    preflight_connection(&conn)
}

pub(crate) fn preflight_connection(conn: &Connection) -> Result<(), AppError> {
    if Database::get_user_version(conn)? > super::SCHEMA_VERSION
        || super::loongport_schema::read_stored_version(conn)?
            > super::loongport_schema::LOONGPORT_SCHEMA_VERSION
    {
        return Err(AppError::Config("secret.database_version_too_new".into()));
    }
    if super::loongport_schema::read_stored_version(conn)? >= 24 && stored_metadata(conn)?.is_none()
    {
        return Err(AppError::Config("secret.metadata_missing".into()));
    }
    Ok(())
}

pub(crate) fn stored_metadata(
    conn: &Connection,
) -> Result<Option<crate::secrets::VaultMetadata>, AppError> {
    use rusqlite::OptionalExtension;
    if !Database::table_exists(conn, "loongport_vault")? {
        return Ok(None);
    }
    let raw: Option<String> = conn
        .query_row("SELECT metadata FROM loongport_vault WHERE id=1", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db_error)?;
    raw.map(|raw| {
        serde_json::from_str(&raw).map_err(|_| AppError::Config("secret.invalid_metadata".into()))
    })
    .transpose()
}

pub(crate) fn check_identity(conn: &Connection, vault: &VaultContext) -> Result<(), AppError> {
    let metadata = stored_metadata(conn)?
        .ok_or_else(|| AppError::Config("secret.migration_required".into()))?;
    if &metadata != vault.metadata() {
        return Err(AppError::Config("secret.identity_mismatch".into()));
    }
    Ok(())
}

pub(crate) fn stamp(conn: &Connection, vault: &VaultContext) -> Result<(), AppError> {
    create_schema(conn)?;
    let metadata = serde_json::to_string(vault.metadata())
        .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    conn.execute(
        "INSERT OR REPLACE INTO loongport_vault(id,metadata) VALUES (1,?1)",
        [metadata],
    )
    .map_err(db_error)?;
    Ok(())
}

pub(crate) fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS loongport_vault (id INTEGER PRIMARY KEY CHECK (id=1), metadata TEXT NOT NULL)").map_err(db_error)
}

pub(crate) fn copy(source: &Connection, destination: &mut Connection) -> Result<(), AppError> {
    let backup = rusqlite::backup::Backup::new(source, destination).map_err(db_error)?;
    Database::complete_backup(&backup, "Copy credential database")
}

pub(crate) fn upgrade_staging(conn: &Connection, vault: &VaultContext) -> Result<(), AppError> {
    if Database::get_user_version(conn)? > super::SCHEMA_VERSION
        || super::loongport_schema::read_stored_version(conn)?
            > super::loongport_schema::LOONGPORT_SCHEMA_VERSION
    {
        return Err(AppError::Config("secret.database_version_too_new".into()));
    }
    let source = stored_metadata(conn)?
        .map(|metadata| {
            if metadata.vault_id != vault.metadata().vault_id
                || metadata.key_id != vault.metadata().key_id
            {
                return Err(AppError::Config("secret.source_key_required".into()));
            }
            VaultContext::from_key(metadata, vault.export_key())
                .map_err(crate::secrets::inventory::secret_error)
        })
        .transpose()?;
    Database::create_tables_on_conn(conn)?;
    Database::apply_schema_migrations_on_conn(conn)?;
    super::loongport_schema::apply(conn)?;
    crate::secrets::inventory::transform_database(conn, source.as_ref(), vault)?;
    stamp(conn, vault)
}

/// The published encrypted staging file is the recovery intent. It remains until
/// SQLite has committed and removed its old journal/pages; no services run meanwhile.
pub(crate) fn prepare(path: &Path, vault: &VaultContext) -> Result<Connection, AppError> {
    preflight(path)?;
    let root = path
        .parent()
        .ok_or_else(|| AppError::Config("secret.invalid_path".into()))?;
    crate::config::ensure_private_directory(root)?;
    let stage_path = root.join(".vault-migration.db");
    if !stage_path.exists() {
        let mut staged = Connection::open_in_memory().map_err(db_error)?;
        if path.exists() {
            let source =
                Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(db_error)?;
            if stored_metadata(&source)?.is_some() {
                check_identity(&source, vault)?;
                crate::secrets::inventory::validate_database(&source, vault)?;
                drop(source);
                crate::config::ensure_private_file(path)?;
                return Connection::open(path).map_err(db_error);
            }
            copy(&source, &mut staged)?;
        }
        upgrade_staging(&staged, vault)?;
        // VACUUM before serialization removes plaintext free pages in the memory image.
        staged
            .execute_batch("PRAGMA secure_delete=ON; VACUUM;")
            .map_err(db_error)?;
        let temporary = tempfile::NamedTempFile::new_in(root).map_err(|e| AppError::io(root, e))?;
        let mut encrypted = Connection::open(temporary.path()).map_err(db_error)?;
        copy(&staged, &mut encrypted)?;
        drop(encrypted);
        temporary
            .as_file()
            .sync_all()
            .map_err(|e| AppError::io(temporary.path(), e))?;
        temporary
            .persist(&stage_path)
            .map_err(|e| AppError::io(&stage_path, e.error))?;
        sync_directory(root)?;
    }
    let staged =
        Connection::open_with_flags(&stage_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(db_error)?;
    check_identity(&staged, vault)?;
    crate::secrets::inventory::validate_database(&staged, vault)?;
    crate::config::ensure_private_file(path)?;
    let mut destination = Connection::open(path).map_err(db_error)?;
    destination
        .execute_batch("PRAGMA secure_delete=ON;")
        .map_err(db_error)?;
    copy(&staged, &mut destination)?;
    destination
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE; VACUUM;")
        .map_err(db_error)?;
    // VACUUM 以改名重建数据库文件：重建产物带的是临时目录的 DACL，且按名打开
    // 有短暂沉降窗口——先重新收紧，再用带重试的 fsync 落盘。
    crate::config::ensure_private_file(path)?;
    crate::config::sync_private_file(path)?;
    let backup_dir = root.join("backups");
    std::fs::create_dir_all(&backup_dir).map_err(|e| AppError::io(&backup_dir, e))?;
    let recovery = backup_dir.join(format!("vault-migration-{}.db", uuid::Uuid::new_v4()));
    drop(staged);
    std::fs::rename(&stage_path, &recovery).map_err(|e| AppError::io(&stage_path, e))?;
    sync_directory(&backup_dir)?;
    sync_directory(root)?;
    Ok(destination)
}

#[cfg_attr(not(unix), allow(unused_variables))]
fn sync_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| AppError::io(path, e))?;
    Ok(())
}

pub(crate) fn migrate_backups(
    root: &Path,
    vault: &VaultContext,
    legacy_allowed: bool,
) -> Result<(), AppError> {
    let directory = root.join("backups");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io(&directory, e)),
    };
    crate::config::ensure_private_directory(&directory)?;
    for entry in entries {
        let entry = entry.map_err(|e| AppError::io(&directory, e))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|e| AppError::io(&path, e))?
            .is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("db")
        {
            continue;
        }
        let source = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(db_error)?;
        if let Some(metadata) = stored_metadata(&source)? {
            if metadata.vault_id != vault.metadata().vault_id
                || metadata.key_id != vault.metadata().key_id
            {
                return Err(AppError::Config("secret.source_key_required".into()));
            }
            let backup_vault = crate::secrets::VaultContext::from_key(metadata, vault.export_key())
                .map_err(crate::secrets::inventory::secret_error)?;
            crate::secrets::inventory::validate_database(&source, &backup_vault)?;
            drop(source);
            crate::config::ensure_private_file(&path)?;
            continue;
        }
        if !legacy_allowed {
            return Err(AppError::Config("secret.plaintext_backup".into()));
        }
        let mut memory = Connection::open_in_memory().map_err(db_error)?;
        copy(&source, &mut memory)?;
        drop(source);
        upgrade_staging(&memory, vault)?;
        memory
            .execute_batch("PRAGMA secure_delete=ON; VACUUM;")
            .map_err(db_error)?;
        let temporary =
            tempfile::NamedTempFile::new_in(&directory).map_err(|e| AppError::io(&directory, e))?;
        let mut destination = Connection::open(temporary.path()).map_err(db_error)?;
        copy(&memory, &mut destination)?;
        drop(destination);
        temporary
            .as_file()
            .sync_all()
            .map_err(|e| AppError::io(temporary.path(), e))?;
        temporary
            .persist(&path)
            .map_err(|e| AppError::io(&path, e.error))?;
        sync_directory(&directory)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_migration_stage_is_resumed_and_authenticated_before_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.db");
        let stage = dir.path().join(".vault-migration.db");
        let vault = VaultContext::generate().unwrap();
        let conn = prepare(&path, &vault).unwrap();
        let encrypted = crate::secrets::inventory::seal_db(
            &vault,
            "settings",
            "value",
            &["global_proxy_url"],
            "https://fixture.invalid",
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES ('global_proxy_url',?1)",
            [encrypted],
        )
        .unwrap();
        drop(conn);
        std::fs::copy(&path, &stage).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute("DELETE FROM settings WHERE key='global_proxy_url'", [])
            .unwrap();
        drop(conn);
        let resumed = prepare(&path, &vault).unwrap();
        assert_eq!(
            resumed
                .query_row(
                    "SELECT count(*) FROM settings WHERE key='global_proxy_url'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert!(!stage.exists());
        drop(resumed);
        std::fs::copy(&path, &stage).unwrap();
        let staged = Connection::open(&stage).unwrap();
        staged
            .execute(
                "UPDATE settings SET value='tampered' WHERE key='global_proxy_url'",
                [],
            )
            .unwrap();
        drop(staged);
        let before = std::fs::read(&path).unwrap();
        assert!(prepare(&path, &vault).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn legacy_upgrade_removes_plaintext_from_database_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.db");
        let conn = Connection::open(&path).unwrap();
        Database::create_tables_on_conn(&conn).unwrap();
        Database::apply_schema_migrations_on_conn(&conn).unwrap();
        super::super::loongport_schema::apply(&conn).unwrap();
        conn.execute(
            "UPDATE loongport_schema_version SET version=23 WHERE id=1",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO providers (id,app_type,name,settings_config,meta) VALUES ('fixture','codex','Fixture',?1,'{}')", [r#"{"api_key":"upgrade-canary-credential"}"#]).unwrap();
        drop(conn);
        let vault = VaultContext::generate().unwrap();
        let migrated = prepare(&path, &vault).unwrap();
        let raw: String = migrated
            .query_row("SELECT settings_config FROM providers", [], |r| r.get(0))
            .unwrap();
        let value = crate::secrets::inventory::open_db(
            &vault,
            "providers",
            "settings_config",
            &["fixture", "codex"],
            &raw,
        )
        .unwrap();
        assert!(value.contains("upgrade-canary-credential"));
        drop(migrated);
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                continue;
            }
            let bytes = std::fs::read(path).unwrap();
            assert!(!bytes
                .windows(b"upgrade-canary-credential".len())
                .any(|w| w == b"upgrade-canary-credential"));
        }
        let reopened = prepare(&path, &vault).unwrap();
        let after: String = reopened
            .query_row("SELECT settings_config FROM providers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, after);
        drop(reopened);
        let before = std::fs::read(&path).unwrap();
        assert!(prepare(&path, &VaultContext::generate().unwrap()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
