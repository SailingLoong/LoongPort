//! Caller credentials are independent of upstream provider authentication.

#[cfg(test)]
mod tests {
    use crate::database::Database;

    #[test]
    fn fresh_database_has_client_credentials() {
        let db = Database::memory().unwrap();
        let conn = db.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='loongport_client_credentials'", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
    }
}

use serde::Serialize;
mod policy;
mod store;
pub(crate) const CLIENT_KEY_HEADER: &str = "x-loongport-client-key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallerIdentity {
    pub client_id: String,
    pub credential_id: String,
    pub route_app: String,
}

pub(crate) use store::{create_schema, validate_schema};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CredentialRef {
    pub id: String,
    pub client_id: String,
    pub route_app: String,
    pub allowed_models: Vec<String>,
    pub revoked: bool,
}

#[cfg(test)]
mod credential_tests {
    use crate::database::Database;

    #[test]
    fn credentials_are_scoped_revocable_and_never_serialized_as_secrets() {
        let db = Database::memory().unwrap();
        let credential = db
            .issue_client_credential("workbuddy", "codex", &["model-a".into()])
            .unwrap();
        let key = db
            .resolve_client_credential(&credential.id, "workbuddy", "codex")
            .unwrap();
        assert!(!serde_json::to_string(&credential).unwrap().contains(&key));
        assert!(db
            .resolve_client_credential(&credential.id, "another-client", "codex")
            .is_err());
        assert!(db
            .authenticate_client(&key, "codex", Some("model-a"))
            .is_ok());
        assert!(db
            .authenticate_client(&key, "claude", Some("model-a"))
            .is_err());
        assert!(db
            .authenticate_client(&key, "codex", Some("model-b"))
            .is_err());
        assert!(db
            .authenticate_client("PROXY_MANAGED", "codex", Some("model-a"))
            .is_err());
        db.revoke_client_credential(&credential.id).unwrap();
        assert!(db
            .resolve_client_credential(&credential.id, "workbuddy", "codex")
            .is_err());
        assert!(db
            .authenticate_client(&key, "codex", Some("model-a"))
            .is_err());
    }
}

#[cfg(test)]
mod protection_tests {
    use crate::{database::Database, secrets::inventory};
    #[test]
    fn client_secrets_follow_vault_export_and_reset_inventory() {
        let db = Database::memory().unwrap();
        let reference = db
            .issue_client_credential("workbuddy", "codex", &["model-a".into()])
            .unwrap();
        let secret = db
            .resolve_client_credential(&reference.id, "workbuddy", "codex")
            .unwrap();
        let conn = db.conn.lock().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT secret FROM loongport_client_credentials WHERE id=?1",
                [&reference.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stored.starts_with("lpenc1."));
        assert!(!stored.contains(&secret));
        let vault = db.secrets.read().unwrap();
        let rotated = vault.rotate_key().unwrap();
        inventory::transform_database(&conn, Some(&vault), &rotated).unwrap();
        let mut exported = None;
        inventory::collect_plaintext_values(&conn, &rotated, |table, _, _, value| {
            if table == "loongport_client_credentials" {
                exported = Some(value);
            }
        })
        .unwrap();
        assert_eq!(exported.as_deref(), Some(secret.as_str()));
        inventory::reset_database(&conn, &rotated).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM loongport_client_credentials",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
