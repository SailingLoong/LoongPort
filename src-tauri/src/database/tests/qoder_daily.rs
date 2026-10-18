//! Actual crate consumers: full Database/SecretSession/Provider and temporary SQLite.
//! No generated implementation blocks, fixture host types, global credentials,
//! installed official device, or external HTTP endpoint are used.
use crate::{database::Database, error, provider, provider::Provider, secrets};
use rusqlite::{params, Connection};
use secrets::{session::SecretSession, VaultContext};
use std::sync::{Arc, Mutex};

struct Fixture {
    path: std::path::PathBuf,
    session: Arc<SecretSession>,
}
impl Fixture {
    fn new() -> Self {
        // The production schema/vault initializer is authoritative. A SQLite
        // snapshot makes reopening and two independent connections observable.
        let seed = Database::memory().unwrap();
        seed.save_provider("codex", &provider("synthetic-generation-1", None))
            .unwrap();
        let session = seed.secrets.clone();
        let path = session.root().join("qoder-regression.sqlite");
        {
            let source = seed.conn.lock().unwrap();
            let mut target = Connection::open(&path).unwrap();
            let copy = rusqlite::backup::Backup::new(&source, &mut target).unwrap();
            Database::complete_backup(&copy, "synthetic Qoder fixture snapshot").unwrap();
        }
        Self { path, session }
    }
    fn open(&self) -> Database {
        let conn = Connection::open(&self.path).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        Database::from_connection(conn, self.session.clone())
    }
    fn load(&self, db: &Database) -> Provider {
        db.get_provider_by_id("synthetic-provider", "codex")
            .unwrap()
            .unwrap()
    }
    fn save(&self, db: &Database, p: &Provider, expected: &str) -> Result<(), error::AppError> {
        let vault = self.session.read()?;
        db.save_provider_with_vault(
            "codex",
            p,
            Some(expected),
            false,
            self.session.as_ref(),
            &vault,
        )
    }
}
fn provider(generation: &str, state: Option<&str>) -> Provider {
    Provider {
        id: "synthetic-provider".into(),
        name: "Synthetic Qoder fixture".into(),
        settings_config: serde_json::json!({"env":{"OPENAI_API_KEY":format!("non-secret-fixture-{generation}")}}),
        available_models: None,
        website_url: None,
        category: None,
        created_at: None,
        sort_index: None,
        notes: state.map(str::to_owned),
        meta: Some(provider::ProviderMeta {
            provider_type: Some("qoder_sdk".into()),
            ..Default::default()
        }),
        icon: None,
        icon_color: None,
        in_failover_queue: false,
    }
}
fn digest(p: &Provider) -> String {
    Database::provider_update_digest(p).unwrap()
}

include!("qoder_daily/service.rs");
include!("qoder_daily/receipts.rs");
include!("qoder_daily/http_clock.rs");
include!("qoder_daily/http_adapter.rs");

#[test]
fn unknown_wire_outcome_keeps_its_existing_contract() {
    assert_eq!(
        serde_json::to_value(Outcome::Unknown).unwrap(),
        serde_json::json!("OutcomeUnknown")
    );
}
