//! Plaintext database identity, bound to the explicit session choice.
//! This table is a consistency token, never a competing mode-choice owner.
use crate::{error::AppError, secrets::storage::PlaintextIdentity};
use rusqlite::{Connection, OptionalExtension};

pub(crate) const TABLE: &str = "loongport_plaintext_storage";

fn invalid() -> AppError {
    AppError::Config("protection.database_identity_invalid".into())
}
fn exists(conn: &Connection, table: &str) -> Result<bool, AppError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )?)
}

pub(crate) fn reject_encrypted(conn: &Connection) -> Result<(), AppError> {
    if exists(conn, "loongport_vault")? {
        // Any encrypted row, including malformed metadata, belongs to the
        // encrypted owner. An empty schema table is created by the original
        // LoongPort migration and carries no mode choice.
        let present = conn
            .query_row("SELECT metadata FROM loongport_vault LIMIT 1", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        if present.is_some() {
            return Err(AppError::Config("protection.encrypted_data_present".into()));
        }
    }
    Ok(())
}

pub(crate) fn stored(conn: &Connection) -> Result<Option<PlaintextIdentity>, AppError> {
    if !exists(conn, TABLE)? {
        return Ok(None);
    }
    let mut stmt = conn.prepare("PRAGMA table_info(loongport_plaintext_storage)")?;
    let fields = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if fields
        != vec![
            ("id".into(), "INTEGER".into(), 0, None, 1),
            ("identity".into(), "TEXT".into(), 1, None, 0),
        ]
    {
        return Err(invalid());
    }
    let extensions: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE tbl_name=?1 AND type IN ('trigger','index')",
        [TABLE],
        |row| row.get(0),
    )?;
    let foreign: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_foreign_key_list('loongport_plaintext_storage')",
        [],
        |row| row.get(0),
    )?;
    if extensions != 0 || foreign != 0 {
        return Err(invalid());
    }
    let mut stmt = conn.prepare("SELECT id,identity FROM loongport_plaintext_storage")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let [(1, raw)] = rows.as_slice() else {
        // An empty or modified identity table is not a legacy database.
        return Err(invalid());
    };
    let identity: PlaintextIdentity = serde_json::from_str(raw).map_err(|_| invalid())?;
    identity.validate().map_err(|_| invalid())?;
    Ok(Some(identity))
}

pub(crate) fn check(conn: &Connection, identity: &PlaintextIdentity) -> Result<(), AppError> {
    identity.validate().map_err(|_| invalid())?;
    reject_encrypted(conn)?;
    let stored = stored(conn)?
        .ok_or_else(|| AppError::Config("protection.database_identity_required".into()))?;
    if stored != *identity {
        return Err(AppError::Config(
            "protection.database_identity_mismatch".into(),
        ));
    }
    Ok(())
}

/// Setup-owned unpublished staging only. Existing identities are never replaced.
pub(crate) fn stamp(conn: &Connection, identity: &PlaintextIdentity) -> Result<(), AppError> {
    identity.validate().map_err(|_| invalid())?;
    reject_encrypted(conn)?;
    if stored(conn)?.is_some() {
        return check(conn, identity);
    }
    let encoded = serde_json::to_string(identity).map_err(|_| invalid())?;
    conn.execute_batch("CREATE TABLE loongport_plaintext_storage (id INTEGER PRIMARY KEY CHECK (id=1), identity TEXT NOT NULL)")?;
    conn.execute(
        "INSERT INTO loongport_plaintext_storage(id,identity) VALUES (1,?1)",
        [encoded],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "storage_identity_tests.rs"]
mod tests;
