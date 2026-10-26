//! Explicit private-copy generation conversion. Publication remains lifecycle-owned.
use super::{storage, storage_identity, vault, Database};
use crate::{
    error::AppError,
    secrets::{inventory, storage::StorageContext},
};
use rusqlite::{config::DbConfig, Connection, TransactionBehavior};

fn validate_context(context: &StorageContext) -> Result<(), AppError> {
    match context {
        StorageContext::Encrypted(vault) => vault.metadata().validate(),
        StorageContext::Plaintext(identity) => identity.validate(),
    }
    .map_err(inventory::secret_error)
}

fn validate_generation(current: &StorageContext, next: &StorageContext) -> Result<(), AppError> {
    validate_context(current)?;
    validate_context(next)?;
    let before = current.identity();
    let after = next.identity();
    if before.store_id() != after.store_id()
        || before.generation_id() == after.generation_id()
        || before.revision().checked_add(1) != Some(after.revision())
    {
        return Err(AppError::Config("protection.invalid_generation".into()));
    }
    Ok(())
}

fn copy_private_image(source: &Connection) -> Result<Connection, AppError> {
    let mut image = Connection::open_in_memory()?;
    vault::copy(source, &mut image)?;
    Ok(image)
}

/// The caller retains the original session-write and database guards, and alone
/// owns publication. This function never writes or changes connection flags on
/// the source; it returns only a completely verified unpublished SQLite image.
pub(crate) fn prepare_storage_generation(
    source: &Connection,
    current: &StorageContext,
    next: &StorageContext,
) -> Result<Connection, AppError> {
    validate_generation(current, next)?;
    prepare_private_image(copy_private_image(source)?, current, next)
}

/// An older backup may carry a previous password-wrapper revision, but it must
/// authenticate with the live generation's actual key. A plaintext backup has
/// no equivalent proof, so its complete consistency identity must match exactly.
fn backup_identity_mismatch() -> AppError {
    AppError::Config("protection.backup_identity_mismatch".into())
}
fn authenticate_backup_context(
    image: &Connection,
    current: &StorageContext,
) -> Result<StorageContext, AppError> {
    let plaintext = storage_identity::stored(image)?;
    match current {
        StorageContext::Plaintext(current) => {
            let source = plaintext.ok_or_else(backup_identity_mismatch)?;
            if &source != current {
                return Err(backup_identity_mismatch());
            }
            storage_identity::check(image, current)?;
            Ok(StorageContext::Plaintext(source))
        }
        StorageContext::Encrypted(current) => {
            if plaintext.is_some() {
                return Err(backup_identity_mismatch());
            }
            let source = vault::stored_metadata(image)?.ok_or_else(backup_identity_mismatch)?;
            if source.vault_id != current.metadata().vault_id
                || source.key_id != current.metadata().key_id
                || source.revision > current.metadata().revision
            {
                return Err(backup_identity_mismatch());
            }
            crate::secrets::VaultContext::from_key(source, current.export_key())
                .map(StorageContext::from)
                .map_err(inventory::secret_error)
        }
    }
}

// The same original copied image supplies both its identity and actual key
// verifier. An optional caller expectation adds a check, never another owner.
fn authenticated_backup_image(
    source: &Connection,
    expected: Option<&StorageContext>,
    current: &StorageContext,
) -> Result<(Connection, StorageContext), AppError> {
    if let Some(expected) = expected {
        validate_context(expected)?;
    }
    let image = copy_private_image(source)?;
    if vault::is_prefork_relic(&image)? {
        return Err(AppError::Config(
            "protection.backup_legacy_unsupported".into(),
        ));
    }
    if let Some(expected) = expected {
        storage::check_identity(&image, expected)?;
    }
    let authenticated = authenticate_backup_context(&image, current)?;
    Ok((image, authenticated))
}

/// Unpublished backup conversion; the complete-generation owner enumerates the
/// original backup inventory and publishes only after every image is prepared.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn prepare_storage_backup_generation(
    source: &Connection,
    source_context: &StorageContext,
    current: &StorageContext,
    next: &StorageContext,
) -> Result<Connection, AppError> {
    validate_generation(current, next)?;
    let (image, authenticated) = authenticated_backup_image(source, Some(source_context), current)?;
    prepare_private_image(image, &authenticated, next)
}

// Connection flags belong only to an unpublished copy and are restored on both
// success and failure. No caller-owned source connection reaches this helper.
pub(super) fn with_private_triggers_disabled<T>(
    image: &mut Connection,
    operation: impl FnOnce(&mut Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let triggers_enabled = image.db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)?;
    if image.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)? {
        return Err(AppError::Config(
            "protection.trigger_state_unavailable".into(),
        ));
    }
    let result = operation(image);
    let restored = image.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, triggers_enabled);
    let output = result?;
    if restored? != triggers_enabled {
        return Err(AppError::Config(
            "protection.trigger_state_unavailable".into(),
        ));
    }
    Ok(output)
}

// Both primary and backup conversion use exactly this admitted private-image
// owner: original inventory walker, complete readback, trigger suppression and
// physical-page cleanup. No export/reset projection or runtime stamping fallback.
pub(super) fn prepare_private_image(
    mut image: Connection,
    current: &StorageContext,
    next: &StorageContext,
) -> Result<Connection, AppError> {
    vault::preflight_structure(&image)?;
    storage::check_identity(&image, current)?;
    let values = inventory::capture_storage_values(&image, current)?;
    image.execute_batch("PRAGMA secure_delete=ON")?;
    with_private_triggers_disabled(&mut image, |image| {
        let tx = image.transaction_with_behavior(TransactionBehavior::Immediate)?;
        inventory::transform_storage_database(&tx, current, next)?;
        // Identity changes belong only to this authenticated private image.
        if storage_identity::stored(&tx)?.is_some() {
            tx.execute_batch("DROP TABLE loongport_plaintext_storage")?;
        }
        if Database::table_exists(&tx, "loongport_vault")? {
            tx.execute("DELETE FROM loongport_vault WHERE id=1", [])?;
        }
        storage::stamp(&tx, next)?;
        storage::check_identity(&tx, next)?;
        values.verify(&tx, next)?;
        tx.commit()?;
        image.execute_batch("VACUUM")?;
        storage::check_identity(image, next)?;
        values.verify(image, next)
    })?;
    Ok(image)
}

/// Admit an actual backup image under the live generation; callers do not own
/// metadata interpretation, key authentication or source-context construction.
pub(crate) fn prepare_storage_backup_generation_from_image(
    source: &Connection,
    current: &StorageContext,
    next: &StorageContext,
) -> Result<Connection, AppError> {
    validate_generation(current, next)?;
    let (image, authenticated) = authenticated_backup_image(source, None, current)?;
    prepare_private_image(image, &authenticated, next)
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "storage_backup_image_tests.rs"]
mod backup_image_tests;
#[cfg(feature = "test-hooks")]
pub(crate) fn verify_backup_image_case(case: &str) {
    backup_image_tests::verify(case);
}

/// Read-only complete logical comparison for an authenticated same-pending image.
pub(crate) fn verify_storage_generation_image(
    source: &Connection,
    current: &StorageContext,
    target: &Connection,
    next: &StorageContext,
) -> Result<(), AppError> {
    validate_generation(current, next)?;
    verify_private_images(
        copy_private_image(source)?,
        current,
        copy_private_image(target)?,
        next,
    )
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "storage_generation_verify_tests.rs"]
mod generation_verify_tests;
#[cfg(feature = "test-hooks")]
pub(crate) fn verify_generation_image_case(case: &str) {
    generation_verify_tests::verify(case);
}

/// Backup-only equality additionally admits an authenticated older same-key source.
pub(crate) fn verify_storage_backup_generation_image(
    source: &Connection,
    current: &StorageContext,
    target: &Connection,
    next: &StorageContext,
) -> Result<(), AppError> {
    validate_generation(current, next)?;
    let (image, authenticated) = authenticated_backup_image(source, None, current)?;
    verify_private_images(image, &authenticated, copy_private_image(target)?, next)
}

fn verify_private_images(
    source: Connection,
    current: &StorageContext,
    target: Connection,
    next: &StorageContext,
) -> Result<(), AppError> {
    vault::preflight_structure(&source)?;
    vault::preflight_structure(&target)?;
    storage::check_identity(&source, current)?;
    storage::check_identity(&target, next)?;
    inventory::capture_storage_values(&source, current)?.verify(&target, next)?;
    if normalized_content_digest(source, current)? != normalized_content_digest(target, next)? {
        return Err(AppError::Config(
            "protection.transformation_mismatch".into(),
        ));
    }
    Ok(())
}

fn normalized_content_digest(
    mut image: Connection,
    context: &StorageContext,
) -> Result<String, AppError> {
    with_private_triggers_disabled(&mut image, |image| {
        // Only known identity schemas are exempt; never drop an unknown column,
        // index or trigger merely because it is attached to an identity table.
        normalize_owned_identities(image, context)?;
        inventory::normalize_storage_values_for_digest(image, context)?;
        Database::content_digest(image)
    })
}

fn normalize_owned_identities(
    image: &Connection,
    context: &StorageContext,
) -> Result<(), AppError> {
    let canonical = Connection::open_in_memory()?;
    vault::create_schema(&canonical)?;
    if let Some(identity) = storage_identity::stored(image)? {
        storage_identity::stamp(&canonical, &identity)?;
    }
    type SchemaObject = (String, String, String, Option<String>);
    fn objects(conn: &Connection, table: &str) -> Result<Vec<SchemaObject>, AppError> {
        let mut statement = conn.prepare(
            "SELECT type,name,tbl_name,sql FROM sqlite_master WHERE tbl_name=?1 ORDER BY type,name",
        )?;
        let rows = statement
            .query_map([table], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    for table in ["loongport_vault", storage_identity::TABLE] {
        if !Database::table_exists(image, table)? {
            continue;
        }
        if objects(image, table)? != objects(&canonical, table)? {
            return Err(AppError::Config(
                "protection.database_identity_invalid".into(),
            ));
        }
        if table == "loongport_vault" {
            let rows: i64 =
                image.query_row("SELECT count(*) FROM loongport_vault", [], |row| row.get(0))?;
            let expected = if context.is_plaintext() { 0 } else { 1 };
            if rows != expected {
                return Err(AppError::Config(
                    "protection.database_identity_invalid".into(),
                ));
            }
        }
        image.execute_batch(&format!("DROP TABLE {table}"))?;
    }
    Ok(())
}
