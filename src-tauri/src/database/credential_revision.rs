//! Credential incarnations belong to the original account rows, never public DTOs.
//! Empty defaults admit old/credential-free inserts but cannot admit shared auth.
use crate::error::AppError;
use rusqlite::Connection;

const COLUMNS: [(&str, &str); 2] = [
    ("providers", "credential_revision"),
    ("loongport_vendor", "api_key_revision"),
];

fn denied() -> AppError {
    AppError::Config("credential_revision.invalid_schema".into())
}

pub(crate) fn new_revision() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn valid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == value)
}

/// Used only by fresh construction / the original versioned migration owner.
pub(crate) fn create_columns(conn: &Connection) -> Result<(), AppError> {
    for (table, column) in COLUMNS {
        let mut statement = conn.prepare(&format!("PRAGMA table_xinfo({table})"))?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !names.iter().any(|name| name == column) {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} TEXT NOT NULL DEFAULT '';"
            ))?;
        }
    }
    validate_schema(conn)
}

pub(crate) fn validate_schema(conn: &Connection) -> Result<(), AppError> {
    for (table, column) in COLUMNS {
        let mut statement = conn.prepare(&format!("PRAGMA table_xinfo({table})"))?;
        let columns = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns
            .iter()
            .any(|(name, kind, required, default, pk, hidden)| {
                name == column
                    && kind.eq_ignore_ascii_case("TEXT")
                    && *required == 1
                    && default.as_deref() == Some("''")
                    && *pk == 0
                    && *hidden == 0
            })
        {
            return Err(denied());
        }
        let mut values = conn.prepare(&format!("SELECT {column} FROM {table}"))?;
        for value in values.query_map([], |row| row.get::<_, String>(0))? {
            let value = value?;
            if !value.is_empty() && !valid(&value) {
                return Err(denied());
            }
        }
    }
    Ok(())
}

/// Pre-31 rows have no trustworthy issuance history. Initialize each independently.
/// The caller holds the original schema savepoint, including its version stamp.
pub(crate) fn migrate(conn: &Connection) -> Result<(), AppError> {
    create_columns(conn)?;
    let providers = conn
        .prepare("SELECT id, app_type FROM providers")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, app) in providers {
        conn.execute(
            "UPDATE providers SET credential_revision=?1 WHERE id=?2 AND app_type=?3",
            rusqlite::params![new_revision(), id, app],
        )?;
    }
    let vendors = conn
        .prepare("SELECT id FROM loongport_vendor")?
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in vendors {
        conn.execute(
            "UPDATE loongport_vendor SET api_key_revision=?1 WHERE id=?2",
            rusqlite::params![new_revision(), id],
        )?;
    }
    validate_schema(conn)
}

/// Compatibility of the original provider writer/planner, not shared-source admission.
/// Authenticated retained checkpoints can legitimately keep the original 24..30
/// schema. They have neither issuance column; never create/migrate one on a read.
/// Modern or mixed/corrupt schemas cannot use this historical branch.
pub(crate) fn original_provider_uses_revisions(conn: &Connection) -> Result<bool, AppError> {
    let version = super::loongport_schema::read_stored_version(conn)?;
    super::vault::preflight_structure(conn)?;
    if (super::loongport_schema::MIN_AUTHENTICATED_SNAPSHOT_VERSION..31).contains(&version) {
        for (table, column) in COLUMNS {
            if !super::Database::table_exists(conn, table)? {
                return Err(denied());
            }
            let mut statement = conn.prepare(&format!("PRAGMA table_xinfo({table})"))?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            if names.iter().any(|name| name.eq_ignore_ascii_case(column)) {
                return Err(denied());
            }
        }
        return Ok(false);
    }
    // The modern preflight above already validates both authoritative columns.
    // Keep pre-authenticated/fresh construction paths strict as well.
    if version < 31 {
        validate_schema(conn)?;
    }
    Ok(true)
}

pub(crate) fn provider_on(conn: &Connection, app: &str, id: &str) -> Result<String, AppError> {
    let revision: String = conn.query_row(
        "SELECT credential_revision FROM providers WHERE id=?1 AND app_type=?2",
        rusqlite::params![id, app],
        |row| row.get(0),
    )?;
    if !valid(&revision) {
        return Err(denied());
    }
    Ok(revision)
}

/// Original authenticated journal proof; never accepted in a frontend Provider DTO.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteProof {
    pub(crate) before: String,
    pub(crate) next: String,
}
impl WriteProof {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if !valid(&self.before) || !valid(&self.next) || self.before == self.next {
            return Err(AppError::Config("credential_revision.invalid_write".into()));
        }
        Ok(())
    }
}

/// Compare only the original auth material, not labels/models/route preferences.
/// The explicit reissue intent is independent of equality and is handled by the writer.
pub(crate) fn authentication_changed(
    app: &str,
    before: &crate::provider::Provider,
    after: &crate::provider::Provider,
) -> bool {
    let Ok(app) = app.parse::<crate::app_config::AppType>() else {
        return before.settings_config != after.settings_config;
    };
    let Some(adapter) = crate::proxy::providers::get_adapter(&app) else {
        return before.settings_config != after.settings_config;
    };
    let auth = |provider: &crate::provider::Provider| {
        adapter
            .extract_auth(provider)
            .map(|auth| (auth.api_key, auth.strategy, auth.access_token))
    };
    auth(before) != auth(after)
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
#[path = "credential_revision_tests.rs"]
mod tests;
#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub(crate) fn verify_fixture(case: &str) {
    tests::verify(case);
}
