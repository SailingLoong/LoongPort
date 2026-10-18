//! Informational projection of the original applied account chain, never spend authority.
use super::{bindings::id, contract::digest};
use crate::{database::Database, error::AppError};
use serde_json::{json, Value};

pub(crate) fn snapshot_on_connection(
    conn: &rusqlite::Connection,
    app: &str,
    owner: &str,
    identity: &str,
) -> Result<Value, AppError> {
    if !id(owner) || !id(app) {
        return Err(AppError::Config("paperclip.snapshot_unavailable".into()));
    }
    let catalog = Database::configuration_catalog_revision_on_connection(conn, app)?;
    let chain = crate::proxy::application_routing::chain_ids_on(conn, app)?;
    let revision = digest(
        b"paperclip-applied-chain-v3\0",
        &serde_json::to_vec(&(owner, app, &catalog, &chain, identity))
            .map_err(|_| AppError::Config("paperclip.snapshot_unavailable".into()))?,
    );
    let resource_id = resource_id(owner, app);
    Ok(
        json!({"version":3,"ownerId":owner,"revision":revision,"executionMode":"estimated-budget","resources":[{"resourceId":resource_id,"nativeSupport":app=="codex"&&!chain.is_empty(),"reason":if app!="codex" {"unsupported_app"} else if chain.is_empty(){"applied_chain_empty"}else{"facts_checked_before_each_send"}}]}),
    )
}
pub(crate) fn resource_id(owner: &str, app: &str) -> String {
    digest(
        b"paperclip-default-applied-chain\0",
        &serde_json::to_vec(&(owner, app)).unwrap(),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_has_no_quota_or_spending_authority() {
        let db = Database::memory().unwrap();
        let c = db.conn.lock().unwrap();
        let v = snapshot_on_connection(&c, "codex", "owner", "session").unwrap();
        assert!(v["resources"][0].get("quota").is_none());
        assert!(v["resources"][0].get("balance").is_none());
    }
    #[test]
    fn original_chain_or_identity_changes_invalidate_revision() {
        let db = Database::memory().unwrap();
        let c = db.conn.lock().unwrap();
        let a = snapshot_on_connection(&c, "codex", "owner", "session").unwrap();
        let b = snapshot_on_connection(&c, "codex", "owner", "other-session").unwrap();
        assert_ne!(a["revision"], b["revision"]);
        c.execute("INSERT OR REPLACE INTO settings(key,value) VALUES('application_priority_codex','[\"candidate\"]')",[]).unwrap();
        let c = snapshot_on_connection(&c, "codex", "owner", "session").unwrap();
        assert_ne!(a["revision"], c["revision"]);
        assert_eq!(
            a["resources"][0]["resourceId"],
            c["resources"][0]["resourceId"]
        );
    }
}
