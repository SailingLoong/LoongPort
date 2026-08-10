use std::sync::Arc;

use crate::{
    database::{lock_conn, Database},
    error::AppError,
};

#[allow(async_fn_in_trait)]
pub(crate) trait TakeoverControl: Send + Sync {
    async fn set_takeover_for_app(&self, app_type: &str, enabled: bool) -> Result<(), String>;
}

impl TakeoverControl for crate::services::ProxyService {
    async fn set_takeover_for_app(&self, app_type: &str, enabled: bool) -> Result<(), String> {
        crate::services::ProxyService::set_takeover_for_app(self, app_type, enabled).await
    }
}

fn table_exists(db: &Database, table: &str) -> Result<bool, String> {
    let conn = lock_conn!(db.conn);
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(|error| format!("failed to inspect legacy model verification tables: {error}"))
}

pub(crate) async fn cleanup_legacy_runtime<P: TakeoverControl>(
    db: &Arc<Database>,
    proxy: &P,
) -> Result<(), String> {
    if table_exists(db, "model_verification_settings")? {
        let conn = lock_conn!(db.conn);
        conn.execute(
            "UPDATE model_verification_settings SET runtime_auto_enabled = 0 WHERE singleton = 1",
            [],
        )
        .map_err(|error| format!("failed to disable legacy runtime verification: {error}"))?;
    }

    if !table_exists(db, "model_verification_proxy_leases")? {
        return Ok(());
    }

    let leases = {
        let conn = lock_conn!(db.conn);
        let mut statement = conn
            .prepare("SELECT app_type FROM model_verification_proxy_leases ORDER BY app_type")
            .map_err(|error| format!("failed to read legacy proxy leases: {error}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| format!("failed to read legacy proxy leases: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to read legacy proxy leases: {error}"))?
    };

    let mut failures = Vec::new();
    for app_type in leases {
        match proxy.set_takeover_for_app(&app_type, false).await {
            Ok(()) => {
                let conn = lock_conn!(db.conn);
                conn.execute(
                    "DELETE FROM model_verification_proxy_leases WHERE app_type = ?1",
                    [&app_type],
                )
                .map_err(|error| {
                    format!("restored {app_type} but failed to delete its legacy lease: {error}")
                })?;
            }
            Err(error) => failures.push(format!("{app_type}: {error}")),
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "failed to restore legacy model verification proxy leases: {}",
            failures.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::database::Database;

    use super::{cleanup_legacy_runtime, TakeoverControl};

    #[derive(Default)]
    struct FakeTakeover {
        calls: Mutex<Vec<(String, bool)>>,
        fail_for: Mutex<Option<String>>,
    }

    impl FakeTakeover {
        fn fail_for(&self, app_type: &str) {
            *self.fail_for.lock().unwrap() = Some(app_type.to_string());
        }
    }

    impl TakeoverControl for FakeTakeover {
        async fn set_takeover_for_app(&self, app_type: &str, enabled: bool) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push((app_type.to_string(), enabled));
            if self.fail_for.lock().unwrap().as_deref() == Some(app_type) {
                Err("restore failed".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn insert_lease(db: &Database, app_type: &str) {
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO model_verification_proxy_leases (app_type, acquired_at) VALUES (?1, 1)",
                [app_type],
            )
            .unwrap();
    }

    fn has_lease(db: &Database, app_type: &str) -> bool {
        db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM model_verification_proxy_leases WHERE app_type = ?1)",
                [app_type],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn missing_legacy_tables_are_a_noop() {
        let db = Arc::new(Database::memory().unwrap());
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("DROP TABLE model_verification_proxy_leases", [])
                .unwrap();
            conn.execute("DROP TABLE model_verification_settings", [])
                .unwrap();
        }
        let fake = FakeTakeover::default();

        cleanup_legacy_runtime(&db, &fake).await.unwrap();

        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn successful_restore_deletes_only_owned_leases() {
        let db = Arc::new(Database::memory().unwrap());
        insert_lease(&db, "codex");
        let fake = FakeTakeover::default();

        cleanup_legacy_runtime(&db, &fake).await.unwrap();

        assert_eq!(
            fake.calls.lock().unwrap().as_slice(),
            [("codex".to_string(), false)]
        );
        assert!(!has_lease(&db, "codex"));
    }

    #[tokio::test]
    async fn failed_restore_keeps_the_lease_for_retry() {
        let db = Arc::new(Database::memory().unwrap());
        insert_lease(&db, "claude");
        let fake = FakeTakeover::default();
        fake.fail_for("claude");

        let error = cleanup_legacy_runtime(&db, &fake).await.unwrap_err();

        assert!(error.contains("claude"));
        assert!(has_lease(&db, "claude"));
    }

    #[tokio::test]
    async fn user_owned_takeover_without_a_lease_is_untouched() {
        let db = Arc::new(Database::memory().unwrap());
        let fake = FakeTakeover::default();

        cleanup_legacy_runtime(&db, &fake).await.unwrap();

        assert!(fake.calls.lock().unwrap().is_empty());
    }
}
