//! Storage-mode admission around the original SQLite/schema owners.
//! Durable user choice remains owned solely by SecretSession/protection.json.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) use super::storage_generation::prepare_storage_backup_generation;
pub(crate) use super::storage_generation::{
    prepare_storage_backup_generation_from_image, prepare_storage_generation,
    verify_storage_backup_generation_image, verify_storage_generation_image,
};
use super::{inspection, storage_identity, vault, Database};
use crate::{
    error::AppError,
    secrets::{
        inventory,
        storage::{PlaintextIdentity, StorageContext},
    },
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::path::Path;

fn runtime_structure(conn: &Connection) -> Result<(), AppError> {
    vault::preflight_structure(conn)?;
    let version = Database::get_user_version(conn)?;
    if version > super::UPSTREAM4_SOURCE_SCHEMA_VERSION && version < super::UPSTREAM4_SCHEMA_VERSION
    {
        return Err(AppError::Config("upgrade.unsupported_source".into()));
    }
    Ok(())
}

/// Passive format inspection only; this cannot select or admit a runtime mode.
/// The session's explicit choice is checked again by `prepare` before use.
pub(crate) fn preflight_runtime(path: &Path) -> Result<(), AppError> {
    let Some(captured) = inspection::capture(path)? else {
        return Ok(());
    };
    let result = (|| {
        if let Some(identity) = storage_identity::stored(&captured.image)? {
            runtime_structure(&captured.image)?;
            storage_identity::check(&captured.image, &identity)?;
            inventory::validate_storage_database(
                &captured.image,
                &StorageContext::Plaintext(identity),
            )?;
        } else {
            vault::preflight_connection(&captured.image)?;
            runtime_structure(&captured.image)?;
        }
        Ok(())
    })();
    inspection::verify_unchanged(path, &captured.revision)?;
    result
}

pub(crate) fn check_identity(conn: &Connection, context: &StorageContext) -> Result<(), AppError> {
    match context {
        StorageContext::Encrypted(v) => {
            if storage_identity::stored(conn)?.is_some() {
                return Err(AppError::Config("protection.plaintext_data_present".into()));
            }
            vault::check_identity(conn, v)
        }
        StorageContext::Plaintext(identity) => storage_identity::check(conn, identity),
    }
}

pub(crate) fn stamp(conn: &Connection, context: &StorageContext) -> Result<(), AppError> {
    match context {
        StorageContext::Encrypted(v) => {
            if storage_identity::stored(conn)?.is_some() {
                return Err(AppError::Config("protection.plaintext_data_present".into()));
            }
            vault::stamp(conn, v)
        }
        StorageContext::Plaintext(identity) => storage_identity::stamp(conn, identity),
    }
}

pub(crate) fn prepare(path: &Path, context: &StorageContext) -> Result<Connection, AppError> {
    match context {
        StorageContext::Encrypted(v) => vault::prepare(path, v),
        StorageContext::Plaintext(identity) => prepare_plaintext(path, identity),
    }
}

fn reject_encrypted_files(root: &Path) -> Result<(), AppError> {
    for name in ["vault.json", ".vault-migration.db"] {
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => return Err(AppError::Config("protection.encrypted_data_present".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::io(root.join(name), error)),
        }
    }
    Ok(())
}

fn prepare_plaintext(path: &Path, identity: &PlaintextIdentity) -> Result<Connection, AppError> {
    identity.validate().map_err(inventory::secret_error)?;
    let root = path
        .parent()
        .ok_or_else(|| AppError::Config("secret.invalid_path".into()))?;
    reject_encrypted_files(root)?;
    let context = StorageContext::Plaintext(identity.clone());
    if let Some(captured) = inspection::capture(path)? {
        // Authenticate the explicit mode identity and validate all protected
        // columns in the private captured image before opening the original RW.
        storage_identity::check(&captured.image, identity)?;
        runtime_structure(&captured.image)?;
        inventory::validate_storage_database(&captured.image, &context)?;
        inspection::verify_unchanged(path, &captured.revision)?;
        let primary = captured.revision.primary_identity()?;
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        inspection::verify_connection_primary_identity(&conn, &primary)?;
        // Keep the original SQLite writer exclusion across the recheck and
        // schema migration. No lock/epoch/lease owner is replaced by this seam.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        storage_identity::check(&tx, identity)?;
        runtime_structure(&tx)?;
        inventory::validate_storage_database(&tx, &context)?;
        prepare_plaintext_staging(&tx, identity)?;
        tx.commit()?;
        inspection::verify_connection_primary_identity(&conn, &primary)?;
        crate::config::ensure_private_file(path)?;
        return Ok(conn);
    }

    // A new DB is prepared completely before publication. Never use SQLite's
    // CREATE flag on the destination or replace a concurrently appeared file.
    let staged = Connection::open_in_memory()?;
    prepare_plaintext_staging(&staged, identity)?;
    crate::config::ensure_private_directory(root)?;
    let temporary = tempfile::NamedTempFile::new_in(root).map_err(|e| AppError::io(root, e))?;
    let mut destination = Connection::open(temporary.path())?;
    vault::copy(&staged, &mut destination)?;
    drop(destination);
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| AppError::io(temporary.path(), e))?;
    reject_encrypted_files(root)?;
    temporary
        .persist_noclobber(path)
        .map_err(|e| AppError::io(path, e.error))?;
    crate::config::ensure_private_file(path)?;
    crate::config::sync_private_file(path)?;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    check_identity(&conn, &context)?;
    inventory::validate_storage_database(&conn, &context)?;
    Ok(conn)
}

/// Explicit setup-owned unpublished staging. This is never an identity-missing
/// restart fallback; the caller retains/publishes the source through its owner.
pub(crate) fn prepare_plaintext_staging(
    conn: &Connection,
    identity: &PlaintextIdentity,
) -> Result<(), AppError> {
    let target = if Database::get_user_version(conn)? == super::UPSTREAM4_SCHEMA_VERSION {
        super::UPSTREAM4_SCHEMA_VERSION
    } else {
        super::UPSTREAM4_SOURCE_SCHEMA_VERSION
    };
    prepare_plaintext_staging_for_target(conn, identity, target)
}

/// Original import owner remains the only publisher of this private image.
pub(crate) fn upgrade_staging(conn: &Connection, context: &StorageContext) -> Result<(), AppError> {
    match context {
        StorageContext::Encrypted(v) => vault::upgrade_staging(conn, v),
        StorageContext::Plaintext(identity) => {
            prepare_plaintext_staging_for_target(conn, identity, super::SCHEMA_VERSION)
        }
    }
}

pub(super) fn prepare_plaintext_staging_for_target(
    conn: &Connection,
    identity: &PlaintextIdentity,
    target: i32,
) -> Result<(), AppError> {
    identity.validate().map_err(inventory::secret_error)?;
    storage_identity::reject_encrypted(conn)?;
    vault::preflight_structure(conn)?;
    if storage_identity::stored(conn)?.is_some() {
        storage_identity::check(conn, identity)?;
    } else if super::loongport_schema::read_stored_version(conn)?
        >= super::loongport_schema::MIN_AUTHENTICATED_SNAPSHOT_VERSION
    {
        // This schema already claimed authenticated storage. A missing row is
        // damaged identity, not a legacy plaintext source, even during setup.
        return Err(AppError::Config(
            "protection.database_identity_required".into(),
        ));
    }
    conn.execute_batch("SAVEPOINT loongport_plaintext_prepare")?;
    let result = (|| {
        Database::create_tables_on_conn(conn)?;
        if target == super::UPSTREAM4_SOURCE_SCHEMA_VERSION {
            Database::apply_upgrade_source_migrations_on_conn(conn)?;
        } else {
            Database::apply_schema_migrations_on_conn(conn)?;
        }
        super::loongport_schema::apply(conn)?;
        let context = StorageContext::Plaintext(identity.clone());
        inventory::validate_storage_database(conn, &context)?;
        storage_identity::stamp(conn, identity)?;
        check_identity(conn, &context)
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("RELEASE loongport_plaintext_prepare")?;
            Ok(())
        }
        Err(error) => {
            conn.execute_batch(
                "ROLLBACK TO loongport_plaintext_prepare; RELEASE loongport_plaintext_prepare",
            )?;
            Err(error)
        }
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) use super::storage_generation::verify_backup_image_case;

#[cfg(feature = "test-hooks")]
pub(crate) use super::storage_generation::verify_generation_image_case;

/// Fresh private empty image through the original mode/schema staging owners.
pub(crate) fn empty_storage_image(context: &StorageContext) -> Result<Connection, AppError> {
    let image = Connection::open_in_memory()?;
    match context {
        StorageContext::Plaintext(identity) => prepare_plaintext_staging(&image, identity)?,
        StorageContext::Encrypted(vault) => super::vault::upgrade_staging_for_target(
            &image,
            vault,
            super::UPSTREAM4_SOURCE_SCHEMA_VERSION,
        )?,
    }
    check_identity(&image, context)?;
    Ok(image)
}

/// Compare a complete empty image without treating the original non-secret
/// DEFAULT datetime('now') values as a persistent generation identity.
pub(crate) fn verify_empty_storage_image(
    image: &Connection,
    context: &StorageContext,
) -> Result<(), AppError> {
    check_identity(image, context)?;
    inventory::validate_storage_database(image, context)?;
    let mut private = Connection::open_in_memory()?;
    vault::copy(image, &mut private)?;
    let mut canonical = empty_storage_image(context)?;
    normalize_empty_default_times(&mut private)?;
    normalize_empty_default_times(&mut canonical)?;
    if Database::content_digest(&private)? != Database::content_digest(&canonical)? {
        return Err(AppError::Config("protection.generation_changed".into()));
    }
    Ok(())
}

fn normalize_empty_default_times(image: &mut Connection) -> Result<(), AppError> {
    let invalid = || AppError::Config("protection.generation_changed".into());
    let times = {
        let mut statement = image
            .prepare("SELECT created_at, updated_at FROM proxy_config")
            .map_err(|_| invalid())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| invalid())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid())?;
        rows
    };
    for (created, updated) in times {
        let parsed = chrono::NaiveDateTime::parse_from_str(&created, "%Y-%m-%d %H:%M:%S")
            .map_err(|_| invalid())?;
        if created != updated
            || created.len() != 19
            || parsed.format("%Y-%m-%d %H:%M:%S").to_string() != created
            || parsed.and_utc().timestamp_subsec_nanos() != 0
        {
            return Err(invalid());
        }
    }
    // Historical v6 seeds this disabled singleton with unix_seconds(); v7
    // retires runtime verification but intentionally retains its row/schema.
    // Admit only that exact default shape and a real nonnegative integer clock.
    let legacy_default = {
        let mut statement = image
            .prepare("SELECT singleton, runtime_auto_enabled, updated_at FROM model_verification_settings")
            .map_err(|_| invalid())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|_| invalid())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid())?;
        rows
    };
    let [(1, 0, updated)] = legacy_default.as_slice() else {
        return Err(invalid());
    };
    if *updated < 0 || chrono::DateTime::<chrono::Utc>::from_timestamp(*updated, 0).is_none() {
        return Err(invalid());
    }
    // Only these declared creation defaults are normalized in private copies.
    // Original schema, every other column/row and unknown objects stay in the
    // complete content digest. Trigger suppression cannot affect the caller.
    super::storage_generation::with_private_triggers_disabled(image, |image| {
        image.execute(
            "UPDATE proxy_config SET created_at=?1, updated_at=?1",
            ["2000-01-01 00:00:00"],
        )?;
        image.execute(
            "UPDATE model_verification_settings SET updated_at=0 WHERE singleton=1",
            [],
        )?;
        Ok(())
    })
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "storage_empty_image_tests.rs"]
mod empty_image_tests;
#[cfg(feature = "test-hooks")]
impl Database {
    #[doc(hidden)]
    pub fn verify_empty_storage_image_case(case: &str) {
        empty_image_tests::verify(case);
    }
}
