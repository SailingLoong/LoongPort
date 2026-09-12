//! Observed forwarding attempts, separate from request accounting and billing.
//! No historical backfill: request logs omit recovered failures. Success means
//! an upstream response was accepted by the forwarding layer; later stream
//! consumption is outside this attempt boundary.
use crate::{
    database::{lock_conn, Database},
    error::AppError,
};
use rusqlite::{params, Connection};
use std::collections::HashMap;

pub(crate) const ATTEMPT_WINDOW_SECS: i64 = 7 * 86400;

pub(crate) fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS provider_attempt_outcomes (
            app_type TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            completed_at INTEGER NOT NULL,
            success INTEGER NOT NULL CHECK (success IN (0,1))
        );
        CREATE INDEX IF NOT EXISTS idx_provider_attempt_app_time
            ON provider_attempt_outcomes(app_type, completed_at);
        CREATE INDEX IF NOT EXISTS idx_provider_attempt_time
            ON provider_attempt_outcomes(completed_at);",
    )?;
    Ok(())
}

impl Database {
    /// Capture one forwarding attempt, including a failure recovered by another
    /// provider or a same-provider rectifier retry. Never adds request/cost rows.
    pub(crate) fn record_provider_attempt(
        &self,
        app: &str,
        provider: &str,
        success: bool,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp();
        let mut conn = lock_conn!(self.conn);
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO provider_attempt_outcomes (app_type, provider_id, completed_at, success) VALUES (?1,?2,?3,?4)", params![app, provider, now, success])?;
        tx.execute(
            "DELETE FROM provider_attempt_outcomes WHERE completed_at < ?1",
            [now - ATTEMPT_WINDOW_SECS],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Only measured attempts contribute; absent providers have unknown rates.
    pub(crate) fn provider_attempt_error_rates(
        &self,
        app: &str,
    ) -> Result<HashMap<String, f64>, AppError> {
        let now = chrono::Utc::now().timestamp();
        let conn = lock_conn!(self.conn);
        let mut stmt = conn.prepare(
            "SELECT provider_id, SUM(1 - success) * 1.0 / COUNT(*)
             FROM provider_attempt_outcomes
             WHERE app_type = ?1 AND completed_at >= ?2 AND completed_at <= ?3
             GROUP BY provider_id",
        )?;
        let rates = stmt
            .query_map(params![app, now - ATTEMPT_WINDOW_SECS, now], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(rates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempts_are_windowed_isolated_and_do_not_change_billing() {
        let db = Database::memory().unwrap();
        assert!(db
            .provider_attempt_error_rates("claude")
            .unwrap()
            .is_empty());
        db.record_provider_attempt("claude", "a", false).unwrap();
        db.record_provider_attempt("claude", "a", true).unwrap();
        db.record_provider_attempt("codex", "a", false).unwrap();
        let now = chrono::Utc::now().timestamp();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO provider_attempt_outcomes VALUES ('claude', 'a', ?1, 0)",
                [now - ATTEMPT_WINDOW_SECS - 10],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO provider_attempt_outcomes VALUES ('claude', 'a', ?1, 0)",
                [now + 1000],
            )
            .unwrap();
            let billing: i64 = conn
                .query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(billing, 0);
        }
        assert_eq!(
            db.provider_attempt_error_rates("claude").unwrap().get("a"),
            Some(&0.5)
        );
        assert_eq!(
            db.provider_attempt_error_rates("codex").unwrap().get("a"),
            Some(&1.0)
        );
        db.record_provider_attempt("claude", "b", true).unwrap();
        let expired: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM provider_attempt_outcomes WHERE completed_at < ?1",
                [now - ATTEMPT_WINDOW_SECS],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(expired, 0);
    }
}
