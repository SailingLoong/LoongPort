//! Recent explicit provider selections. Reading the overview never changes history.
use crate::{
    app_config::AppType,
    database::{lock_conn, Database},
    error::AppError,
};
use rusqlite::{params, OptionalExtension};

const RECENT_LIMIT: usize = 8;
fn history_key(app: &AppType) -> String {
    format!("application_recent_providers_{}", app.as_str())
}

pub fn recent_provider_ids(db: &Database, app: &AppType) -> Result<Vec<String>, AppError> {
    let stored = db.get_setting(&history_key(app))?;
    let ids: Vec<String> = stored
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| AppError::Config(format!("Invalid recent selections: {e}")))?
        .unwrap_or_default();
    let providers = db.get_all_providers(app.as_str())?;
    Ok(ids
        .into_iter()
        .filter(|id| providers.contains_key(id))
        .take(RECENT_LIMIT)
        .collect())
}

fn record_selection(db: &Database, app: &AppType, id: &str) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    let update = || -> Result<(), rusqlite::Error> {
        // Unknown or deleted providers must not become history entries.
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM providers WHERE app_type=?1 AND id=?2)",
            params![app.as_str(), id],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(());
        }
        let key = history_key(app);
        let stored: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key=?1", [&key], |r| {
                r.get(0)
            })
            .optional()?;
        let mut ids: Vec<String> = stored
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
            .unwrap_or_default();
        ids.retain(|previous| previous != id);
        ids.insert(0, id.to_string());
        ids.truncate(RECENT_LIMIT);
        let value = serde_json::to_string(&ids)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)",
            params![key, value],
        )?;
        Ok(())
    };
    update().map_err(|e| AppError::Database(e.to_string()))
}

/// Called only after an explicit selection succeeds, never by automatic routing,
/// provisioning, or the shared provider-switched event emitter.
pub fn record_successful_selection(db: &Database, app: &AppType, id: &str) {
    if let Err(error) = record_selection(db, app, id) {
        // A history write must not report an already-applied configuration as failed.
        log::warn!("Could not save recent provider selection: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    #[test]
    fn recent_selections_are_bounded_deduplicated_isolated_and_exclude_deleted() {
        let db = Database::memory().unwrap();
        for i in 0..10 {
            let provider = Provider::with_id(
                format!("p{i}"),
                format!("Provider {i}"),
                serde_json::json!({}),
                None,
            );
            db.save_provider("codex", &provider).unwrap();
            db.save_provider("claude", &provider).unwrap();
            record_selection(&db, &AppType::Codex, &provider.id).unwrap();
        }
        record_selection(&db, &AppType::Codex, "p3").unwrap();
        let ids = recent_provider_ids(&db, &AppType::Codex).unwrap();
        assert_eq!(ids, ["p3", "p9", "p8", "p7", "p6", "p5", "p4", "p2"]);
        assert!(recent_provider_ids(&db, &AppType::Claude)
            .unwrap()
            .is_empty());
        db.delete_provider("codex", "p3").unwrap();
        record_selection(&db, &AppType::Codex, "missing").unwrap();
        assert_eq!(recent_provider_ids(&db, &AppType::Codex).unwrap()[0], "p9");
        assert_eq!(recent_provider_ids(&db, &AppType::Codex).unwrap().len(), 7);
    }
}
