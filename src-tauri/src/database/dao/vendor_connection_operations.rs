//! Device-local evidence for vendor connection attempts; never stores credentials.
use crate::error::AppError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const CREATE: &str = "CREATE TABLE IF NOT EXISTS vendor_connection_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    storage_binding TEXT NOT NULL,
    account_identity TEXT NOT NULL,
    app TEXT NOT NULL,
    receipt_revision INTEGER NOT NULL CHECK(receipt_revision > 0),
    receipt_json TEXT NOT NULL
)";
const INDEX: &str = "CREATE INDEX IF NOT EXISTS vendor_connection_account ON vendor_connection_operations(storage_binding, account_identity, app)";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Receipt {
    pub schema_version: u32,
    pub operation_id: String,
    pub storage_binding: String,
    pub account_identity: String,
    pub row_id: i64,
    pub vendor_id: String,
    pub plan: String,
    pub app: String,
    pub provider_id: String,
    pub provider_revision: String,
    pub credential_revision: String,
    pub configuration_revision: String,
    #[serde(default)]
    pub completed_configuration_revision: Option<String>,
    pub catalog_decision: Option<crate::services::provider::codex_direct::CatalogDecision>,
    pub prepared_request: Option<crate::services::application_selection::PreparedRoutingRequest>,
    pub state: String,
    pub receipt_revision: u64,
}
fn error(code: &str) -> AppError {
    AppError::Config(format!("vendorConnection.{code}"))
}
fn valid(r: &Receipt) -> bool {
    r.schema_version == 1
        && uuid::Uuid::parse_str(&r.operation_id).is_ok()
        && !r.storage_binding.is_empty()
        && !r.account_identity.is_empty()
        && r.row_id > 0
        && !r.vendor_id.is_empty()
        && !r.plan.is_empty()
        && !r.app.is_empty()
        && !r.provider_id.is_empty()
        && !r.provider_revision.is_empty()
        && !r.configuration_revision.is_empty()
        && r.receipt_revision > 0
        && matches!(
            r.state.as_str(),
            "prepared" | "applying" | "unknown" | "completed" | "failedBeforeWrite" | "reviewed"
        )
}
pub(crate) fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(CREATE)?;
    conn.execute_batch(INDEX)?;
    validate_schema(conn)
}
pub(crate) fn validate_schema(conn: &Connection) -> Result<(), AppError> {
    fn normalized(sql: &str) -> String {
        sql.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .replace("ifnotexists", "")
    }
    for (kind, name, expected) in [
        ("table", "vendor_connection_operations", CREATE),
        ("index", "vendor_connection_account", INDEX),
    ] {
        let actual: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type=?1 AND name=?2",
                params![kind, name],
                |r| r.get(0),
            )
            .optional()?;
        if actual.as_deref().map(normalized) != Some(normalized(expected)) {
            return Err(error("invalidSchema"));
        }
    }
    Ok(())
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PersistedOperation {
    operation: String,
    binding: String,
    account: String,
    app: String,
    revision: i64,
    json: String,
}
fn decode(row: &PersistedOperation) -> Result<Receipt, AppError> {
    let receipt: Receipt = serde_json::from_str(&row.json).map_err(|_| error("invalidReceipt"))?;
    if !valid(&receipt)
        || receipt.operation_id != row.operation
        || receipt.storage_binding != row.binding
        || receipt.account_identity != row.account
        || receipt.app != row.app
        || i64::try_from(receipt.receipt_revision).ok() != Some(row.revision)
    {
        return Err(error("invalidReceipt"));
    }
    Ok(receipt)
}
pub(crate) fn read_rows(conn: &Connection) -> Result<Vec<PersistedOperation>, AppError> {
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='vendor_connection_operations')", [], |r| r.get(0))?;
    if !exists {
        if crate::database::loongport_schema::read_stored_version(conn)? >= 33 {
            return Err(error("invalidSchema"));
        }
        return Ok(Vec::new());
    }
    validate_schema(conn)?;
    let mut stmt = conn.prepare("SELECT operation_id,storage_binding,account_identity,app,receipt_revision,receipt_json FROM vendor_connection_operations ORDER BY rowid")?;
    let rows = stmt.query_map([], |r| {
        Ok(PersistedOperation {
            operation: r.get(0)?,
            binding: r.get(1)?,
            account: r.get(2)?,
            app: r.get(3)?,
            revision: r.get(4)?,
            json: r.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        decode(&row)?;
        out.push(row);
    }
    Ok(out)
}
pub(crate) fn preserve_local(current: &Connection, incoming: &Connection) -> Result<(), AppError> {
    let rows = read_rows(current)?;
    create_schema(incoming)?;
    incoming.execute_batch("SAVEPOINT preserve_vendor_connection")?;
    let result = (|| {
        incoming.execute("DELETE FROM vendor_connection_operations", [])?;
        for r in rows {
            incoming.execute("INSERT INTO vendor_connection_operations(operation_id,storage_binding,account_identity,app,receipt_revision,receipt_json) VALUES(?1,?2,?3,?4,?5,?6)", params![r.operation,r.binding,r.account,r.app,r.revision,r.json])?;
        }
        Ok::<(), AppError>(())
    })();
    if let Err(error) = result {
        incoming.execute_batch(
            "ROLLBACK TO preserve_vendor_connection; RELEASE preserve_vendor_connection",
        )?;
        return Err(error);
    }
    incoming.execute_batch("RELEASE preserve_vendor_connection")?;
    Ok(())
}

pub(crate) fn read_on(conn: &Connection, operation: &str) -> Result<Option<Receipt>, AppError> {
    let row = conn.query_row("SELECT operation_id,storage_binding,account_identity,app,receipt_revision,receipt_json FROM vendor_connection_operations WHERE operation_id=?1", [operation], |r| Ok(PersistedOperation { operation:r.get(0)?, binding:r.get(1)?, account:r.get(2)?, app:r.get(3)?, revision:r.get(4)?, json:r.get(5)? })).optional()?;
    row.as_ref().map(decode).transpose()
}

/// Resolve an outstanding operation without trusting a caller-supplied UUID.
/// A retained row from another store is uncertainty, never absence.
pub(crate) fn unresolved_for_app(
    conn: &Connection,
    binding: &str,
    app: &str,
) -> Result<Option<Receipt>, AppError> {
    if binding.is_empty() || app.is_empty() {
        return Err(error("invalidQuery"));
    }
    let mut found = None;
    for row in read_rows(conn)? {
        let receipt = decode(&row)?;
        if receipt.app != app
            || !matches!(receipt.state.as_str(), "prepared" | "applying" | "unknown")
        {
            continue;
        }
        if receipt.storage_binding != binding {
            return Err(error("storageChanged"));
        }
        if found.is_some() {
            return Err(error("ambiguousOperation"));
        }
        found = Some(receipt);
    }
    Ok(found)
}

/// Read an outstanding ID for explicit review, preserving its original binding.
pub(crate) fn outstanding_for_review(
    conn: &Connection,
    app: &str,
) -> Result<Option<Receipt>, AppError> {
    if app.is_empty() {
        return Err(error("invalidQuery"));
    }
    let mut found = None;
    for row in read_rows(conn)? {
        let receipt = decode(&row)?;
        if receipt.app != app
            || !matches!(receipt.state.as_str(), "prepared" | "applying" | "unknown")
        {
            continue;
        }
        if found.is_some() {
            return Err(error("ambiguousOperation"));
        }
        found = Some(receipt);
    }
    Ok(found)
}

/// Recover the most recent local attempt after the caller loses its ID.
/// This is a local transaction receipt, not proof that a client adopted it.
pub(crate) fn latest_for_app(
    conn: &Connection,
    binding: &str,
    app: &str,
) -> Result<Option<Receipt>, AppError> {
    if binding.is_empty() || app.is_empty() {
        return Err(error("invalidQuery"));
    }
    if let Some(outstanding) = unresolved_for_app(conn, binding, app)? {
        return Ok(Some(outstanding));
    }
    let mut latest = None;
    for row in read_rows(conn)? {
        let receipt = decode(&row)?;
        if receipt.app != app {
            continue;
        }
        latest = Some(receipt);
    }
    if latest
        .as_ref()
        .is_some_and(|receipt| receipt.storage_binding != binding)
    {
        return Err(error("storageChanged"));
    }
    Ok(latest)
}
fn encode(receipt: &Receipt) -> Result<String, AppError> {
    if !valid(receipt) {
        return Err(error("invalidReceipt"));
    }
    serde_json::to_string(receipt).map_err(|_| error("invalidReceipt"))
}
fn same_intent(a: &Receipt, b: &Receipt) -> bool {
    a.operation_id == b.operation_id
        && a.storage_binding == b.storage_binding
        && a.account_identity == b.account_identity
        && a.row_id == b.row_id
        && a.vendor_id == b.vendor_id
        && a.plan == b.plan
        && a.app == b.app
        && a.provider_id == b.provider_id
        && a.provider_revision == b.provider_revision
        && a.credential_revision == b.credential_revision
        && a.configuration_revision == b.configuration_revision
        && matches!(
            (serde_json::to_value(&a.catalog_decision), serde_json::to_value(&b.catalog_decision)),
            (Ok(left), Ok(right)) if left == right
        )
        && matches!(
            (serde_json::to_value(&a.prepared_request), serde_json::to_value(&b.prepared_request)),
            (Ok(left), Ok(right)) if left == right
        )
}
pub(crate) fn prepare_on(conn: &mut Connection, proposed: &Receipt) -> Result<Receipt, AppError> {
    if proposed.state != "prepared" || proposed.receipt_revision != 1 {
        return Err(error("invalidReceipt"));
    }
    let json = encode(proposed)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(existing) = read_on(&tx, &proposed.operation_id)? {
        if !same_intent(&existing, proposed) {
            return Err(error("operationScopeChanged"));
        }
        tx.commit()?;
        return Ok(existing);
    }
    for row in read_rows(&tx)? {
        let existing = decode(&row)?;
        if existing.app == proposed.app
            && matches!(existing.state.as_str(), "prepared" | "applying" | "unknown")
        {
            return Err(error("operationRequiresReconciliation"));
        }
    }
    tx.execute("INSERT INTO vendor_connection_operations(operation_id,storage_binding,account_identity,app,receipt_revision,receipt_json) VALUES(?1,?2,?3,?4,1,?5)", params![proposed.operation_id, proposed.storage_binding, proposed.account_identity, proposed.app, json])?;
    tx.commit()?;
    Ok(proposed.clone())
}
pub(crate) fn transition_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    expected: u64,
    next: &str,
    admit: impl FnOnce(&Connection) -> Result<(), AppError>,
) -> Result<Receipt, AppError> {
    transition_with_configuration_on(conn, binding, operation, expected, next, None, admit)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn transition_with_configuration_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    expected: u64,
    next: &str,
    completed_configuration_revision: Option<String>,
    admit: impl FnOnce(&Connection) -> Result<(), AppError>,
) -> Result<Receipt, AppError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut receipt = read_on(&tx, operation)?.ok_or_else(|| error("missingOperation"))?;
    if receipt.storage_binding != binding {
        return Err(error("storageChanged"));
    }
    if receipt.receipt_revision != expected {
        return Err(error("receiptChanged"));
    }
    let allowed = matches!(
        (receipt.state.as_str(), next),
        ("prepared", "applying")
            | ("prepared", "failedBeforeWrite")
            | ("applying", "completed")
            | ("applying", "unknown")
            | ("unknown", "completed")
            | ("unknown", "reviewed")
    );
    if !allowed {
        return Err(error("invalidTransition"));
    }
    admit(&tx)?;
    receipt.state = next.to_owned();
    receipt.completed_configuration_revision = completed_configuration_revision;
    receipt.receipt_revision = receipt
        .receipt_revision
        .checked_add(1)
        .ok_or_else(|| error("invalidReceipt"))?;
    let json = encode(&receipt)?;
    let changed = tx.execute("UPDATE vendor_connection_operations SET receipt_revision=?1,receipt_json=?2 WHERE operation_id=?3 AND receipt_revision=?4", params![i64::try_from(receipt.receipt_revision).map_err(|_| error("invalidReceipt"))?, json, operation, i64::try_from(expected).map_err(|_| error("invalidReceipt"))?])?;
    if changed != 1 {
        return Err(error("receiptChanged"));
    }
    tx.commit()?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Receipt {
        Receipt {
            schema_version: 1,
            operation_id: "2d75e8d4-9140-45eb-b67d-39ed8c854001".into(),
            storage_binding: "fixture-store".into(),
            account_identity: "fixture-account-digest".into(),
            row_id: 1,
            vendor_id: "opencode".into(),
            plan: "opencode".into(),
            app: "codex".into(),
            provider_id: "fixture-provider".into(),
            provider_revision: "fixture-provider-revision".into(),
            credential_revision: "fixture-key-revision".into(),
            configuration_revision: "fixture-config-revision".into(),
            completed_configuration_revision: None,
            catalog_decision: None,
            prepared_request: None,
            state: "prepared".into(),
            receipt_revision: 1,
        }
    }
    #[test]
    fn lost_receipt_retains_claim_and_rejects_new_intent() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_schema(&conn).unwrap();
        let receipt = fixture();
        prepare_on(&mut conn, &receipt).unwrap();
        let applying = transition_on(
            &mut conn,
            &receipt.storage_binding,
            &receipt.operation_id,
            1,
            "applying",
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(prepare_on(&mut conn, &receipt).unwrap().state, "applying");
        let unknown = transition_on(
            &mut conn,
            &receipt.storage_binding,
            &receipt.operation_id,
            applying.receipt_revision,
            "unknown",
            |_| Ok(()),
        )
        .unwrap();
        let mut next = fixture();
        next.operation_id = "2d75e8d4-9140-45eb-b67d-39ed8c854002".into();
        assert!(prepare_on(&mut conn, &next).is_err());
        assert!(transition_on(
            &mut conn,
            "foreign-store",
            &receipt.operation_id,
            unknown.receipt_revision,
            "reviewed",
            |_| Ok(())
        )
        .is_err());
        assert_eq!(
            read_on(&conn, &receipt.operation_id)
                .unwrap()
                .unwrap()
                .state,
            "unknown"
        );
    }
    #[test]
    fn preserve_local_discards_foreign_operation() {
        let mut local = Connection::open_in_memory().unwrap();
        create_schema(&local).unwrap();
        let mut incoming = Connection::open_in_memory().unwrap();
        create_schema(&incoming).unwrap();
        let receipt = fixture();
        prepare_on(&mut local, &receipt).unwrap();
        let mut foreign = fixture();
        foreign.operation_id = "2d75e8d4-9140-45eb-b67d-39ed8c854003".into();
        foreign.storage_binding = "foreign-store".into();
        prepare_on(&mut incoming, &foreign).unwrap();
        let before = read_rows(&local).unwrap();
        preserve_local(&local, &incoming).unwrap();
        assert!(read_rows(&incoming).unwrap() == before);
    }
    #[test]
    fn installed_missing_ledger_cannot_erase_incoming_evidence() {
        let current = Connection::open_in_memory().unwrap();
        current.execute_batch("CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY, version INTEGER NOT NULL); INSERT INTO loongport_schema_version VALUES(1,33)").unwrap();
        let mut incoming = Connection::open_in_memory().unwrap();
        create_schema(&incoming).unwrap();
        let foreign = fixture();
        prepare_on(&mut incoming, &foreign).unwrap();
        let before = read_rows(&incoming).unwrap();
        assert!(preserve_local(&current, &incoming).is_err());
        assert!(read_rows(&incoming).unwrap() == before);
    }

    #[test]
    fn lost_operation_id_can_be_found_only_under_matching_store_and_app() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_schema(&conn).unwrap();
        let receipt = fixture();
        prepare_on(&mut conn, &receipt).unwrap();
        assert_eq!(
            unresolved_for_app(&conn, &receipt.storage_binding, &receipt.app)
                .unwrap()
                .unwrap()
                .operation_id,
            receipt.operation_id
        );
        assert!(
            unresolved_for_app(&conn, &receipt.storage_binding, "claude")
                .unwrap()
                .is_none()
        );
        assert!(unresolved_for_app(&conn, "foreign-store", &receipt.app).is_err());
    }

    #[test]
    fn lost_completed_receipt_is_findable_and_restore_keeps_order() {
        let mut current = Connection::open_in_memory().unwrap();
        create_schema(&current).unwrap();
        let first = fixture();
        prepare_on(&mut current, &first).unwrap();
        transition_on(
            &mut current,
            &first.storage_binding,
            &first.operation_id,
            1,
            "applying",
            |_| Ok(()),
        )
        .unwrap();
        transition_on(
            &mut current,
            &first.storage_binding,
            &first.operation_id,
            2,
            "completed",
            |_| Ok(()),
        )
        .unwrap();
        let mut next = fixture();
        next.operation_id = "2d75e8d4-9140-45eb-b67d-39ed8c854000".into();
        prepare_on(&mut current, &next).unwrap();
        let incoming = Connection::open_in_memory().unwrap();
        create_schema(&incoming).unwrap();
        preserve_local(&current, &incoming).unwrap();
        assert_eq!(
            latest_for_app(&incoming, &next.storage_binding, &next.app)
                .unwrap()
                .unwrap()
                .operation_id,
            next.operation_id
        );
        assert!(latest_for_app(&incoming, "foreign-store", &next.app).is_err());
    }

    #[test]
    fn changed_store_cannot_bypass_existing_unknown_for_same_app() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_schema(&conn).unwrap();
        let first = fixture();
        prepare_on(&mut conn, &first).unwrap();
        transition_on(
            &mut conn,
            &first.storage_binding,
            &first.operation_id,
            1,
            "applying",
            |_| Ok(()),
        )
        .unwrap();
        transition_on(
            &mut conn,
            &first.storage_binding,
            &first.operation_id,
            2,
            "unknown",
            |_| Ok(()),
        )
        .unwrap();
        let mut next = fixture();
        next.operation_id = "2d75e8d4-9140-45eb-b67d-39ed8c854099".into();
        next.storage_binding = "new-store".into();
        assert!(
            prepare_on(&mut conn, &next).is_err(),
            "a changed store must not erase uncertainty"
        );
    }

    #[test]
    fn explicitly_reviewed_old_store_does_not_mask_new_intent() {
        let mut conn = Connection::open_in_memory().unwrap();
        create_schema(&conn).unwrap();
        let first = fixture();
        prepare_on(&mut conn, &first).unwrap();
        transition_on(
            &mut conn,
            &first.storage_binding,
            &first.operation_id,
            1,
            "applying",
            |_| Ok(()),
        )
        .unwrap();
        transition_on(
            &mut conn,
            &first.storage_binding,
            &first.operation_id,
            2,
            "unknown",
            |_| Ok(()),
        )
        .unwrap();
        transition_on(
            &mut conn,
            &first.storage_binding,
            &first.operation_id,
            3,
            "reviewed",
            |_| Ok(()),
        )
        .unwrap();
        let mut next = fixture();
        next.operation_id = "2d75e8d4-9140-45eb-b67d-39ed8c854100".into();
        next.storage_binding = "new-store".into();
        prepare_on(&mut conn, &next).unwrap();
        assert_eq!(
            latest_for_app(&conn, &next.storage_binding, &next.app)
                .unwrap()
                .unwrap()
                .operation_id,
            next.operation_id
        );
        assert_eq!(
            read_on(&conn, &first.operation_id)
                .unwrap()
                .unwrap()
                .storage_binding,
            first.storage_binding
        );
    }
}
