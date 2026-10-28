//! Non-secret setup facts and create-attempt claims. The existing Database owns
//! the connection/lifecycle; callers must supply its current storage binding.
use crate::error::AppError;
use crate::relay::setup_contract::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

const CREATE: &str = "CREATE TABLE IF NOT EXISTS relay_setup_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    account_identity TEXT NOT NULL,
    storage_binding TEXT NOT NULL,
    receipt_revision INTEGER NOT NULL CHECK(receipt_revision > 0),
    receipt_json TEXT NOT NULL
)";
const INDEX: &str =
    "CREATE INDEX IF NOT EXISTS relay_setup_account ON relay_setup_operations(account_identity)";
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
        ("table", "relay_setup_operations", CREATE),
        ("index", "relay_setup_account", INDEX),
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

fn error(code: &str) -> AppError {
    AppError::Config(format!("relaySetup.{code}"))
}
fn encode(receipt: &RelaySetupReceipt) -> Result<String, AppError> {
    serde_json::to_string(receipt).map_err(|_| error("invalidReceipt"))
}
fn decode(
    operation: &str,
    account: &str,
    revision: i64,
    json: &str,
) -> Result<RelaySetupReceipt, AppError> {
    let receipt: RelaySetupReceipt =
        serde_json::from_str(json).map_err(|_| error("invalidReceipt"))?;
    if receipt.schema_version != 1
        || receipt.operation_id != operation
        || receipt.account_identity != account
        || i64::try_from(receipt.receipt_revision).ok() != Some(revision)
        || revision < 1
    {
        return Err(error("invalidReceipt"));
    }
    Ok(receipt)
}
pub(crate) fn read_on(
    conn: &Connection,
    operation: &str,
) -> Result<Option<RelaySetupReceipt>, AppError> {
    let row:Option<(String,i64,String)>=conn.query_row("SELECT account_identity,receipt_revision,receipt_json FROM relay_setup_operations WHERE operation_id=?1",[operation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    row.map(|(account, revision, json)| decode(operation, &account, revision, &json))
        .transpose()
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PersistedOperation {
    operation: String,
    account: String,
    binding: String,
    revision: i64,
    json: String,
}
pub(crate) fn read_rows(conn: &Connection) -> Result<Vec<PersistedOperation>, AppError> {
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='relay_setup_operations')",[],|r|r.get(0))?;
    if !exists {
        return Ok(Vec::new());
    } // Legacy source; installed schema validates before entry.
    validate_schema(conn)?;
    let mut statement=conn.prepare("SELECT operation_id,account_identity,storage_binding,receipt_revision,receipt_json FROM relay_setup_operations ORDER BY operation_id")?;
    let rows = statement.query_map([], |r| {
        Ok(PersistedOperation {
            operation: r.get(0)?,
            account: r.get(1)?,
            binding: r.get(2)?,
            revision: r.get(3)?,
            json: r.get(4)?,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        let row = row?;
        decode(&row.operation, &row.account, row.revision, &row.json)?;
        if row.binding.is_empty() {
            return Err(error("invalidReceipt"));
        }
        result.push(row);
    }
    Ok(result)
}

pub(crate) fn bound_on(
    conn: &Connection,
    binding: &str,
    operation: &str,
) -> Result<RelaySetupReceipt, AppError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT storage_binding FROM relay_setup_operations WHERE operation_id=?1",
            [operation],
            |r| r.get(0),
        )
        .optional()?;
    if stored.as_deref() != Some(binding) || binding.is_empty() {
        return Err(error("storageChanged"));
    }
    read_on(conn, operation)?.ok_or_else(|| error("missingOperation"))
}
/// Read only: an absent client-side UUID must not make an unresolved attempt unreachable.
pub(crate) fn query_account_on(
    conn: &Connection,
    binding: &str,
    relay_id: i64,
    account_identity: &str,
    operation: Option<&str>,
    history_offset: Option<usize>,
) -> Result<Option<RelaySetupReceipt>, AppError> {
    if binding.is_empty()
        || account_identity.is_empty()
        || (operation.is_some() && history_offset.is_some())
    {
        return Err(error("invalidQuery"));
    }
    if let Some(operation) = operation {
        let Some(receipt) = read_on(conn, operation)? else {
            return Ok(None);
        };
        bound_on(conn, binding, operation)?;
        if receipt.relay_id != relay_id || receipt.account_identity != account_identity {
            return Err(error("operationScopeChanged"));
        }
        return Ok(Some(receipt));
    }
    let mut statement=conn.prepare("SELECT operation_id FROM relay_setup_operations WHERE account_identity=?1 AND storage_binding=?2")?;
    let ids = statement
        .query_map(params![account_identity, binding], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut receipts = Vec::new();
    for id in ids {
        let receipt = bound_on(conn, binding, &id)?;
        if receipt.relay_id == relay_id {
            receipts.push(receipt);
        }
    }
    receipts.sort_by(|a, b| (b.created_at, &b.operation_id).cmp(&(a.created_at, &a.operation_id)));
    if let Some(offset) = history_offset {
        return Ok(receipts.into_iter().nth(offset));
    }
    // Prepared-but-not-connected and known partial failure are still resumable
    // operations. Never equate the resource phase's settled state with applied.
    receipts.retain(|r| {
        r.status != SetupStatus::Settled
            || r.targets
                .iter()
                .any(|t| t.connection.state != ConnectionState::Applied)
    });
    // An unresolved write takes precedence over a later unrelated prepared item.
    if let Some(index) = receipts.iter().position(|r| {
        r.status == SetupStatus::NeedsReconcile
            || r.groups.iter().any(|g| g.state == KeyState::Requesting)
    }) {
        return Ok(Some(receipts.remove(index)));
    }
    Ok(receipts.into_iter().next())
}

/// `inserted=false` means a repeated start: return the old receipt, do not run it.
pub(crate) fn begin_on(
    conn: &mut Connection,
    binding: &str,
    receipt: &RelaySetupReceipt,
) -> Result<(RelaySetupReceipt, bool), AppError> {
    if binding.is_empty()
        || receipt.schema_version != 1
        || receipt.receipt_revision != 1
        || receipt.selections.is_empty()
    {
        return Err(error("invalidReceipt"));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) = read_on(&tx, &receipt.operation_id)? {
        bound_on(&tx, binding, &receipt.operation_id)?;
        if existing.selection_digest != receipt.selection_digest
            || existing.account_identity != receipt.account_identity
            || existing.relay_id != receipt.relay_id
        {
            return Err(error("operationScopeChanged"));
        }
        return Ok((existing, false));
    }
    tx.execute("INSERT INTO relay_setup_operations(operation_id,account_identity,storage_binding,receipt_revision,receipt_json) VALUES(?1,?2,?3,?4,?5)",params![receipt.operation_id,receipt.account_identity,binding,i64::try_from(receipt.receipt_revision).map_err(|_|error("invalidRevision"))?,encode(receipt)?])?;
    tx.commit()?;
    Ok((receipt.clone(), true))
}
fn status(receipt: &RelaySetupReceipt) -> SetupStatus {
    if receipt.groups.iter().any(|g| g.state == KeyState::Unknown)
        || receipt.targets.iter().any(|t| {
            t.configuration == ConfigurationState::Unknown
                || t.connection.state == ConnectionState::Unknown
        })
    {
        SetupStatus::NeedsReconcile
    } else if receipt
        .groups
        .iter()
        .any(|g| g.state == KeyState::Requesting)
        || receipt.targets.iter().any(|t| {
            t.configuration == ConfigurationState::NotStarted
                || t.connection.state == ConnectionState::Applying
        })
    {
        SetupStatus::Running
    } else {
        SetupStatus::Settled
    }
}
fn update_on<F>(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    now: i64,
    change: F,
) -> Result<RelaySetupReceipt, AppError>
where
    F: FnOnce(&Connection, &mut RelaySetupReceipt) -> Result<(), AppError>,
{
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let receipt = update_in_transaction_on(&tx, binding, operation, now, change)?;
    tx.commit()?;
    Ok(receipt)
}
fn update_in_transaction_on<F>(
    conn: &Connection,
    binding: &str,
    operation: &str,
    now: i64,
    change: F,
) -> Result<RelaySetupReceipt, AppError>
where
    F: FnOnce(&Connection, &mut RelaySetupReceipt) -> Result<(), AppError>,
{
    let mut receipt = bound_on(conn, binding, operation)?;
    let previous = receipt.clone();
    change(conn, &mut receipt)?;
    receipt.status = status(&receipt);
    receipt.created_key_count = receipt
        .groups
        .iter()
        .filter(|g| g.origin == Some(KeyOrigin::Created))
        .count();
    if receipt == previous {
        return Ok(receipt);
    }
    receipt.receipt_revision = receipt
        .receipt_revision
        .checked_add(1)
        .ok_or_else(|| error("invalidRevision"))?;
    receipt.updated_at = now.max(receipt.updated_at);
    let updated=conn.execute("UPDATE relay_setup_operations SET receipt_revision=?1,receipt_json=?2 WHERE operation_id=?3 AND receipt_revision=?4",params![i64::try_from(receipt.receipt_revision).map_err(|_|error("invalidRevision"))?,encode(&receipt)?,operation,i64::try_from(previous.receipt_revision).map_err(|_|error("invalidRevision"))?])?;
    if updated != 1 {
        return Err(error("receiptChanged"));
    }
    Ok(receipt)
}
pub(crate) use crate::relay::setup_journal::KeyEvent;
pub(crate) fn key_event_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    group: &str,
    event: KeyEvent,
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |tx, receipt| {
        change_key_event(tx, receipt, operation, group, event)
    })
}
pub(crate) fn key_event_authorized_on(
    conn: &mut Connection,
    authority: &crate::relay::setup_journal::PreparationAuthority,
    group: &str,
    event: KeyEvent,
    now: i64,
    check: impl FnOnce(&Connection) -> Result<(), AppError>,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(
        conn,
        &authority.storage_binding,
        &authority.operation_id,
        now,
        |tx, receipt| {
            check(tx)?;
            change_key_event(tx, receipt, &authority.operation_id, group, event)
        },
    )
}
fn change_key_event(
    tx: &Connection,
    receipt: &mut RelaySetupReceipt,
    operation: &str,
    group: &str,
    event: KeyEvent,
) -> Result<(), AppError> {
    let index = receipt
        .groups
        .iter()
        .position(|g| g.group_id == group)
        .ok_or_else(|| error("selectionChanged"))?;
    if matches!(&event, KeyEvent::BeforeCreate) {
        if !matches!(
            receipt.groups[index].state,
            KeyState::NotStarted | KeyState::Rejected
        ) {
            return Err(error("reconcileRequired"));
        }
        let mut statement=tx.prepare("SELECT receipt_json FROM relay_setup_operations WHERE account_identity=?1 AND operation_id<>?2")?;
        let rows = statement.query_map(params![receipt.account_identity, operation], |r| {
            r.get::<_, String>(0)
        })?;
        for row in rows {
            let other: RelaySetupReceipt =
                serde_json::from_str(&row?).map_err(|_| error("invalidReceipt"))?;
            if other.groups.iter().any(|g| {
                g.group_id == group && matches!(g.state, KeyState::Requesting | KeyState::Unknown)
            }) {
                return Err(error("previousAttemptUnresolved"));
            }
        }
    }
    let fact = &mut receipt.groups[index];
    let observed = matches!(&event, KeyEvent::Observed(_));
    match event {
        KeyEvent::BeforeCreate => {
            fact.state = KeyState::Requesting;
            fact.reason_code = None;
        }
        KeyEvent::Created(remote_id) => {
            if fact.state == KeyState::Ready && fact.origin == Some(KeyOrigin::Created) {
                return Ok(());
            }
            if !matches!(fact.state, KeyState::Requesting | KeyState::Unknown)
                && fact.origin != Some(KeyOrigin::Observed)
            {
                return Err(error("attemptNotStarted"));
            }
            fact.state = KeyState::Ready;
            fact.origin = Some(KeyOrigin::Created);
            fact.remote_key_id = remote_id;
            fact.reason_code = None;
        }
        KeyEvent::Reused(remote_id) | KeyEvent::Observed(remote_id) => {
            if fact.state == KeyState::Ready {
                if fact
                    .remote_key_id
                    .as_ref()
                    .is_some_and(|known| known != &remote_id)
                {
                    return Err(error("keyIdentityChanged"));
                }
                // In particular do not attribute a later list item to a
                // NewAPI create acknowledgement that carried no key ID.
                return Ok(());
            }
            if !observed && matches!(fact.state, KeyState::Unknown | KeyState::Requesting) {
                return Err(error("reconcileRequired"));
            }
            fact.state = KeyState::Ready;
            fact.origin = Some(if observed {
                KeyOrigin::Observed
            } else {
                KeyOrigin::Reused
            });
            fact.remote_key_id = Some(remote_id);
            fact.reason_code = None;
        }
        KeyEvent::OutcomeUnknown => {
            if fact.state == KeyState::Ready {
                return Ok(());
            }
            if !matches!(fact.state, KeyState::Requesting | KeyState::Unknown) {
                return Err(error("attemptNotStarted"));
            }
            fact.state = KeyState::Unknown;
            fact.reason_code = Some("remoteOutcomeUnknown".into());
        }
    }
    Ok(())
}
pub(crate) fn local_fact_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    app: &str,
    fact: ConfigurationState,
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |_, receipt| {
        change_local_fact(receipt, app, fact)
    })
}

fn change_local_fact(
    receipt: &mut RelaySetupReceipt,
    app: &str,
    fact: ConfigurationState,
) -> Result<(), AppError> {
    let index = receipt
        .targets
        .iter()
        .position(|t| t.app == app)
        .ok_or_else(|| error("selectionChanged"))?;
    if matches!(
        receipt.targets[index].configuration,
        ConfigurationState::Existing | ConfigurationState::Prepared
    ) {
        return Ok(());
    }
    if fact == ConfigurationState::NotStarted {
        return Err(error("invalidLocalFact"));
    }
    if fact == ConfigurationState::Prepared
        && !receipt
            .groups
            .iter()
            .any(|g| g.group_id == receipt.targets[index].group_id && g.state == KeyState::Ready)
    {
        return Err(error("keyNotReady"));
    }
    let target = &mut receipt.targets[index];
    target.reason_code = match fact {
        ConfigurationState::Failed => Some("localSaveFailed".into()),
        ConfigurationState::Unknown => Some("localOutcomeUnknown".into()),
        _ => None,
    };
    target.configuration = fact;
    Ok(())
}
pub(crate) fn validate_authority_on(
    conn: &Connection,
    authority: &crate::relay::setup_journal::PreparationAuthority,
    current_storage: &str,
    current_account: &str,
    current_session: &str,
    current_epoch: u64,
) -> Result<RelaySetupReceipt, AppError> {
    if current_storage != authority.storage_binding || current_epoch != authority.lifecycle_epoch {
        return Err(error("storageChanged"));
    }
    if current_account != authority.account_identity
        || current_session != authority.session_fingerprint
    {
        return Err(error("accountChanged"));
    }
    let receipt = bound_on(conn, &authority.storage_binding, &authority.operation_id)?;
    if receipt.account_identity != authority.account_identity {
        return Err(error("accountChanged"));
    }
    Ok(receipt)
}

pub(crate) fn local_fact_in_transaction_on(
    conn: &Connection,
    binding: &str,
    operation: &str,
    app: &str,
    fact: ConfigurationState,
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_in_transaction_on(conn, binding, operation, now, |_, receipt| {
        change_local_fact(receipt, app, fact)
    })
}

pub(crate) fn begin_resume_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    expected_revision: u64,
    apps: &[String],
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |_, receipt| {
        validate_resume(receipt, expected_revision, apps).map_err(error)?;
        for target in receipt
            .targets
            .iter_mut()
            .filter(|target| apps.contains(&target.app))
        {
            target.configuration = ConfigurationState::NotStarted;
            target.compatibility = Compatibility::Unknown;
            target.reason_code = None;
        }
        Ok(())
    })
}

pub(crate) fn preparation_issue_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    group: &str,
    app: Option<&str>,
    issue: crate::relay::setup_journal::PreparationIssue,
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |_, receipt| {
        let mut found = false;
        for target in receipt
            .targets
            .iter_mut()
            .filter(|target| target.group_id == group && app.is_none_or(|app| target.app == app))
        {
            found = true;
            if target.configuration != ConfigurationState::NotStarted {
                continue;
            }
            target.configuration = ConfigurationState::Failed;
            target.reason_code = Some(issue.code().into());
            if matches!(
                issue,
                crate::relay::setup_journal::PreparationIssue::ModelUnsupported
            ) {
                target.compatibility = Compatibility::Unsupported;
            }
        }
        if !found {
            return Err(error("selectionChanged"));
        }
        Ok(())
    })
}

pub(crate) fn model_support_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    group: &str,
    apps: &[String],
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |_, receipt| {
        for target in receipt
            .targets
            .iter_mut()
            .filter(|target| target.group_id == group)
        {
            if target.configuration != ConfigurationState::NotStarted {
                continue;
            }
            if apps.contains(&target.app) {
                target.compatibility = Compatibility::Supported;
            } else {
                target.compatibility = Compatibility::Unsupported;
                target.configuration = ConfigurationState::Failed;
                target.reason_code = Some(
                    crate::relay::setup_journal::PreparationIssue::ModelUnsupported
                        .code()
                        .into(),
                );
            }
        }
        Ok(())
    })
}

pub(crate) fn configuration_review_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    app: &str,
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |_, receipt| {
        let target = receipt
            .targets
            .iter_mut()
            .find(|t| t.app == app)
            .ok_or_else(|| error("selectionChanged"))?;
        if !matches!(
            target.configuration,
            ConfigurationState::Existing | ConfigurationState::Prepared
        ) {
            target.configuration = ConfigurationState::Unknown;
            target.reason_code = Some("needsConfigurationReview".into());
        }
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_connection_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    app: &str,
    expected_revision: u64,
    request_id: &str,
    intent: RelayConnectionIntent,
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |_, receipt| {
        if receipt.receipt_revision != expected_revision {
            return Err(error("receiptChanged"));
        }
        let target = receipt
            .targets
            .iter_mut()
            .find(|t| t.app == app)
            .ok_or_else(|| error("selectionChanged"))?;
        if !matches!(
            target.configuration,
            ConfigurationState::Prepared | ConfigurationState::Existing
        ) || target.compatibility == Compatibility::Unsupported
        {
            return Err(error("configurationNotReady"));
        }
        if matches!(
            target.connection.state,
            ConnectionState::Applying | ConnectionState::Unknown | ConnectionState::Applied
        ) {
            return Err(error("connectionRequiresReconciliation"));
        }
        if request_id.is_empty()
            || intent.before_revision.is_empty()
            || intent.change["selection"]["providerId"].as_str()
                != Some(target.provider_id.as_str())
            || intent.change["order"] != serde_json::Value::Null
            || intent.change["catalogPreviewOnly"] != false
            || intent.change["selection"]["model"] != serde_json::Value::Null
        {
            return Err(error("invalidConnectionIntent"));
        }
        target.connection = RelayConnectionFact {
            state: ConnectionState::ConfirmationRequired,
            request_id: Some(request_id.into()),
            intent: Some(intent),
            quit_chatgpt: None,
            observed_current: None,
            write_acknowledged: None,
            observed_revision: None,
            applied_revision: None,
            reason_code: None,
        };
        Ok(())
    })
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin_connection_authorized_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    app: &str,
    expected_revision: u64,
    quit_chatgpt: Option<bool>,
    now: i64,
    check: impl FnOnce(&Connection) -> Result<(), AppError>,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |tx, receipt| {
        check(tx)?;
        if receipt.receipt_revision != expected_revision {
            return Err(error("receiptChanged"));
        }
        let target = receipt
            .targets
            .iter_mut()
            .find(|t| t.app == app)
            .ok_or_else(|| error("selectionChanged"))?;
        if !matches!(
            target.configuration,
            ConfigurationState::Prepared | ConfigurationState::Existing
        ) || target.connection.state != ConnectionState::ConfirmationRequired
            || target.connection.intent.is_none()
            || target.connection.request_id.is_none()
        {
            return Err(error("connectionRequiresReview"));
        }
        target.connection.state = ConnectionState::Applying;
        target.connection.quit_chatgpt = quit_chatgpt;
        target.connection.reason_code = None;
        Ok(())
    })
}
/// Observations are tied to the same durable attempt; never replace its original
/// routing credential after a lost reply. Only the orchestration owner supplies facts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn connection_result_on(
    conn: &mut Connection,
    binding: &str,
    operation: &str,
    app: &str,
    request_id: &str,
    state: ConnectionState,
    observed_current: Option<bool>,
    observed_revision: Option<String>,
    write_acknowledged: bool,
    reason: Option<&str>,
    now: i64,
) -> Result<RelaySetupReceipt, AppError> {
    update_on(conn, binding, operation, now, |_, receipt| {
        let target = receipt
            .targets
            .iter_mut()
            .find(|t| t.app == app)
            .ok_or_else(|| error("selectionChanged"))?;
        if target.connection.request_id.as_deref() != Some(request_id) {
            return Err(error("connectionRequestChanged"));
        }
        if write_acknowledged {
            target.connection.write_acknowledged = Some(true);
        }
        if state == ConnectionState::Applied && target.connection.applied_revision.is_none() {
            target.connection.applied_revision = observed_revision.clone();
        }
        target.connection.state = state;
        target.connection.observed_current = observed_current;
        target.connection.observed_revision = observed_revision;
        target.connection.reason_code = reason.map(str::to_owned);
        Ok(())
    })
}

/// Restore/sync follows the existing backup owner's local-receipt preservation.
pub(crate) fn preserve_local(current: &Connection, incoming: &Connection) -> Result<(), AppError> {
    let rows = read_rows(current)?;
    create_schema(incoming)?;
    incoming.execute_batch("SAVEPOINT preserve_relay_setup")?;
    let result = (|| {
        incoming.execute("DELETE FROM relay_setup_operations", [])?;
        for row in rows {
            incoming.execute("INSERT INTO relay_setup_operations(operation_id,account_identity,storage_binding,receipt_revision,receipt_json) VALUES(?1,?2,?3,?4,?5)",params![row.operation,row.account,row.binding,row.revision,row.json])?;
        }
        Ok::<_, AppError>(())
    })();
    if let Err(error) = result {
        incoming.execute_batch("ROLLBACK TO preserve_relay_setup; RELEASE preserve_relay_setup")?;
        return Err(error);
    }
    incoming.execute_batch("RELEASE preserve_relay_setup")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receipt(id: &str, group: &str) -> RelaySetupReceipt {
        let d = RelayDiscovery {
            schema_version: 1,
            relay_id: 7,
            account: RelayAccountIdentity {
                identity: "fixture/account7".into(),
                site_origin: "https://fixture.example".into(),
                account_label: "Fixture".into(),
            },
            revision: "r1".into(),
            choices: vec![RelayChoice {
                app: "codex".into(),
                group_id: group.into(),
                group_name: "Same display name".into(),
                provider_id: format!("provider-{group}"),
                local_configuration: LocalConfiguration::Absent,
                compatibility: ChoiceCompatibility::Unknown,
                key_status: UnknownKeyStatus::Unknown,
            }],
        };
        initial_receipt(
            id,
            &[RelaySelection {
                app: "codex".into(),
                group_id: group.into(),
            }],
            &d,
            1,
        )
        .unwrap()
    }

    const A: &str = "f8d02319-67c1-426b-a931-c836de11320e";
    const B: &str = "95fc0852-e7f3-4d3d-bd83-5e333fa49ec4";
    const BIND: &str = "fixture-store/generation";
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        create_schema(&c).unwrap();
        c
    }
    fn authority() -> crate::relay::setup_journal::PreparationAuthority {
        crate::relay::setup_journal::PreparationAuthority {
            operation_id: A.into(),
            storage_binding: BIND.into(),
            account_identity: "fixture/account7".into(),
            session_fingerprint: "session-before-wait".into(),
            lifecycle_epoch: 4,
        }
    }
    fn begin_connection_on(
        conn: &mut Connection,
        binding: &str,
        operation: &str,
        app: &str,
        revision: u64,
        quit: Option<bool>,
        now: i64,
    ) -> Result<RelaySetupReceipt, AppError> {
        begin_connection_authorized_on(conn, binding, operation, app, revision, quit, now, |_| {
            Ok(())
        })
    }
    #[test]
    fn application_admission_failure_does_not_commit_applying_or_consume_confirmation() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        let ready =
            local_fact_on(&mut c, BIND, A, "codex", ConfigurationState::Existing, 2).unwrap();
        let reviewed = prepare_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            ready.receipt_revision,
            "request-a",
            connection_intent(),
            3,
        )
        .unwrap();
        assert!(begin_connection_authorized_on(
            &mut c,
            BIND,
            A,
            "codex",
            reviewed.receipt_revision,
            Some(true),
            4,
            |_| Err(error("accountChanged"))
        )
        .is_err());
        assert_eq!(bound_on(&c, BIND, A).unwrap(), reviewed);
    }
    fn connection_intent() -> RelayConnectionIntent {
        RelayConnectionIntent {
            prepared_request: serde_json::json!({"protocol":"legacy","review":{"draftDigest":"d","revision":"r"}}),
            change: serde_json::json!({"selection":{"providerId":"provider-a","model":null},"order":null,"catalogDecision":null,"catalogPreviewOnly":false}),
            before_revision: "overview-r".into(),
            provider_revision: None,
        }
    }
    #[test]
    fn application_attempt_requires_ready_target_and_exact_review_revision() {
        let mut c = db();
        let (first, _) = begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        assert!(prepare_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            first.receipt_revision,
            "request-a",
            connection_intent(),
            2
        )
        .is_err());
        key_event_on(&mut c, BIND, A, "a", KeyEvent::Reused("key-a".into()), 2).unwrap();
        let ready =
            local_fact_on(&mut c, BIND, A, "codex", ConfigurationState::Prepared, 3).unwrap();
        assert!(prepare_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            first.receipt_revision,
            "request-a",
            connection_intent(),
            4
        )
        .is_err());
        let reviewed = prepare_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            ready.receipt_revision,
            "request-a",
            connection_intent(),
            5,
        )
        .unwrap();
        assert_eq!(
            reviewed.targets[0].connection.state,
            ConnectionState::ConfirmationRequired
        );
        assert!(begin_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            ready.receipt_revision,
            Some(false),
            6
        )
        .is_err());
        let begun = begin_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            reviewed.receipt_revision,
            Some(false),
            7,
        )
        .unwrap();
        assert_eq!(begun.targets[0].connection.state, ConnectionState::Applying);
        assert_eq!(begun.targets[0].connection.quit_chatgpt, Some(false));
        assert_eq!(
            bound_on(&c, BIND, A).unwrap().targets[0].connection.intent,
            Some(connection_intent())
        );
        assert!(begin_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            begun.receipt_revision,
            Some(true),
            8
        )
        .is_err());
        assert!(prepare_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            begun.receipt_revision,
            "replacement",
            connection_intent(),
            9
        )
        .is_err());
    }
    #[test]
    fn uncertain_connection_cannot_be_overwritten_by_a_fresh_request() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        let ready =
            local_fact_on(&mut c, BIND, A, "codex", ConfigurationState::Existing, 2).unwrap();
        let reviewed = prepare_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            ready.receipt_revision,
            "original",
            connection_intent(),
            3,
        )
        .unwrap();
        let unknown = update_on(&mut c, BIND, A, 4, |_, receipt| {
            receipt.targets[0].connection.state = ConnectionState::Unknown;
            Ok(())
        })
        .unwrap();
        assert!(prepare_connection_on(
            &mut c,
            BIND,
            A,
            "codex",
            unknown.receipt_revision,
            "new",
            connection_intent(),
            5
        )
        .is_err());
        let retained = bound_on(&c, BIND, A).unwrap();
        assert_eq!(
            retained.targets[0].connection.request_id,
            reviewed.targets[0].connection.request_id
        );
        assert_eq!(retained.status, SetupStatus::NeedsReconcile);
    }

    #[test]
    fn actual_write_authority_rejects_changed_storage_account_and_waited_session() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        let a = authority();
        validate_authority_on(&c, &a, BIND, "fixture/account7", "session-before-wait", 4).unwrap();
        assert!(validate_authority_on(
            &c,
            &a,
            "other-store",
            "fixture/account7",
            "session-before-wait",
            4
        )
        .is_err());
        assert!(
            validate_authority_on(&c, &a, BIND, "fixture/account7", "session-before-wait", 5)
                .is_err(),
            "same-store restoration invalidates the old execution lease"
        );
        assert!(
            validate_authority_on(&c, &a, BIND, "fixture/account8", "session-before-wait", 4)
                .is_err()
        );
        assert!(
            validate_authority_on(&c, &a, BIND, "fixture/account7", "session-after-wait", 4)
                .is_err()
        );
    }
    #[test]
    fn local_projection_and_prepared_fact_share_one_commit_or_one_rollback() {
        for reject in [false, true] {
            let mut c = db();
            begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
            key_event_on(&mut c, BIND, A, "a", KeyEvent::Reused("1".into()), 2).unwrap();
            c.execute_batch("CREATE TABLE fixture_projection(id TEXT PRIMARY KEY)")
                .unwrap();
            if reject {
                c.execute_batch("CREATE TRIGGER reject_fact BEFORE UPDATE ON relay_setup_operations BEGIN SELECT RAISE(ABORT,'fixture receipt failure'); END;").unwrap();
            }
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            validate_authority_on(
                &tx,
                &authority(),
                BIND,
                "fixture/account7",
                "session-before-wait",
                4,
            )
            .unwrap();
            tx.execute("INSERT INTO fixture_projection VALUES('provider-a')", [])
                .unwrap();
            let fact = local_fact_in_transaction_on(
                &tx,
                BIND,
                A,
                "codex",
                ConfigurationState::Prepared,
                3,
            );
            if reject {
                assert!(fact.is_err());
                drop(tx);
            } else {
                fact.unwrap();
                tx.commit().unwrap();
            }
            let count: i64 = c
                .query_row("SELECT count(*) FROM fixture_projection", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, if reject { 0 } else { 1 });
            assert_eq!(
                read_on(&c, A).unwrap().unwrap().targets[0].configuration,
                if reject {
                    ConfigurationState::NotStarted
                } else {
                    ConfigurationState::Prepared
                }
            );
        }
    }
    #[test]
    fn explicit_resume_starts_only_failed_targets_and_clears_old_attempt_reasons() {
        use crate::relay::setup_journal::PreparationIssue;
        let mut c = db();
        let mut original = receipt(A, "a");
        let mut sibling = original.targets[0].clone();
        sibling.app = "claude".into();
        sibling.provider_id = "other".into();
        sibling.configuration = ConfigurationState::Existing;
        original.targets.push(sibling.clone());
        begin_on(&mut c, BIND, &original).unwrap();
        key_event_on(&mut c, BIND, A, "a", KeyEvent::Reused("1".into()), 2).unwrap();
        let failed = preparation_issue_on(
            &mut c,
            BIND,
            A,
            "a",
            Some("codex"),
            PreparationIssue::ModelUnsupported,
            3,
        )
        .unwrap();
        let resumed = begin_resume_on(
            &mut c,
            BIND,
            A,
            failed.receipt_revision,
            &["codex".into()],
            4,
        )
        .unwrap();
        assert_eq!(
            resumed.targets[0].configuration,
            ConfigurationState::NotStarted
        );
        assert_eq!(resumed.targets[0].reason_code, None);
        assert_eq!(resumed.targets[0].compatibility, Compatibility::Unknown);
        assert_eq!(resumed.targets[1], sibling);
        assert_eq!(resumed.groups, failed.groups);
        assert!(begin_resume_on(
            &mut c,
            BIND,
            A,
            failed.receipt_revision,
            &["codex".into()],
            5
        )
        .is_err());
        let ended = preparation_issue_on(
            &mut c,
            BIND,
            A,
            "a",
            Some("codex"),
            PreparationIssue::PreparationIncomplete,
            6,
        )
        .unwrap();
        assert_eq!(
            ended.targets[0].reason_code.as_deref(),
            Some("preparationIncomplete")
        );
    }
    #[test]
    fn explicit_resume_clears_old_unsupported_reason_before_supported_catalog() {
        use crate::relay::setup_journal::PreparationIssue;
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        let failed = preparation_issue_on(
            &mut c,
            BIND,
            A,
            "a",
            Some("codex"),
            PreparationIssue::ModelUnsupported,
            2,
        )
        .unwrap();
        begin_resume_on(
            &mut c,
            BIND,
            A,
            failed.receipt_revision,
            &["codex".into()],
            3,
        )
        .unwrap();
        let r = model_support_on(&mut c, BIND, A, "a", &["codex".into()], 3).unwrap();
        assert_eq!(r.targets[0].compatibility, Compatibility::Supported);
        assert_eq!(r.targets[0].configuration, ConfigurationState::NotStarted);
        assert_eq!(r.targets[0].reason_code, None);
    }
    #[test]
    fn retry_catalog_does_not_restart_an_unselected_failed_sibling() {
        verify_retry_sibling_scope(true);
    }
    #[test]
    fn retry_group_issue_does_not_overwrite_an_unselected_failed_sibling() {
        verify_retry_sibling_scope(false);
    }
    fn verify_retry_sibling_scope(catalog: bool) {
        use crate::relay::setup_journal::PreparationIssue;
        let mut c = db();
        let mut original = receipt(A, "a");
        let mut sibling = original.targets[0].clone();
        sibling.app = "claude".into();
        sibling.provider_id = "claude-provider".into();
        original.targets.push(sibling);
        begin_on(&mut c, BIND, &original).unwrap();
        key_event_on(&mut c, BIND, A, "a", KeyEvent::Reused("1".into()), 2).unwrap();
        let failed = preparation_issue_on(
            &mut c,
            BIND,
            A,
            "a",
            None,
            PreparationIssue::ModelUnsupported,
            3,
        )
        .unwrap();
        begin_resume_on(
            &mut c,
            BIND,
            A,
            failed.receipt_revision,
            &["claude".into()],
            4,
        )
        .unwrap();
        let after = if catalog {
            model_support_on(&mut c, BIND, A, "a", &["codex".into(), "claude".into()], 5).unwrap()
        } else {
            preparation_issue_on(
                &mut c,
                BIND,
                A,
                "a",
                None,
                PreparationIssue::RemoteReadFailed,
                5,
            )
            .unwrap()
        };
        assert_eq!(after.targets[0], failed.targets[0]);
        assert_eq!(
            after.targets[1].configuration,
            if catalog {
                ConfigurationState::NotStarted
            } else {
                ConfigurationState::Failed
            }
        );
    }
    #[test]
    fn preparation_stage_failures_are_not_fabricated_as_local_save_failures() {
        use crate::relay::setup_journal::PreparationIssue;
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        key_event_on(&mut c, BIND, A, "a", KeyEvent::Reused("1".into()), 2).unwrap();
        let r = preparation_issue_on(
            &mut c,
            BIND,
            A,
            "a",
            Some("codex"),
            PreparationIssue::ModelCatalogUnavailable,
            3,
        )
        .unwrap();
        assert_eq!(
            r.targets[0].reason_code.as_deref(),
            Some("modelCatalogUnavailable")
        );
        assert_eq!(r.targets[0].compatibility, Compatibility::Unknown);
        assert_eq!(r.groups[0].state, KeyState::Ready);
        begin_resume_on(&mut c, BIND, A, r.receipt_revision, &["codex".into()], 4).unwrap();
        let r = preparation_issue_on(
            &mut c,
            BIND,
            A,
            "a",
            Some("codex"),
            PreparationIssue::ModelUnsupported,
            4,
        )
        .unwrap();
        assert_eq!(r.targets[0].compatibility, Compatibility::Unsupported);
        assert_eq!(
            r.targets[0].reason_code.as_deref(),
            Some("modelUnsupported")
        );
        local_fact_on(&mut c, BIND, A, "codex", ConfigurationState::Prepared, 5).unwrap();
        let r = preparation_issue_on(
            &mut c,
            BIND,
            A,
            "a",
            Some("codex"),
            PreparationIssue::LocalSaveFailed,
            6,
        )
        .unwrap();
        assert_eq!(r.targets[0].configuration, ConfigurationState::Prepared);
    }
    #[test]
    fn account_query_recovers_unknown_without_a_cached_operation_id() {
        let mut c = db();
        assert!(
            query_account_on(&c, BIND, 7, "fixture/account7", None, None)
                .unwrap()
                .is_none()
        );
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        key_event_on(&mut c, BIND, A, "a", KeyEvent::BeforeCreate, 2).unwrap();
        key_event_on(&mut c, BIND, A, "a", KeyEvent::OutcomeUnknown, 3).unwrap();
        let found = query_account_on(&c, BIND, 7, "fixture/account7", None, None)
            .unwrap()
            .unwrap();
        assert_eq!(found.operation_id, A);
        assert_eq!(found.groups[0].state, KeyState::Unknown);
        assert!(
            query_account_on(&c, BIND, 8, "fixture/account8", None, None)
                .unwrap()
                .is_none()
        );
        assert!(
            query_account_on(&c, "foreign-storage", 7, "fixture/account7", None, None)
                .unwrap()
                .is_none()
        );
        assert!(
            query_account_on(&c, BIND, 7, "fixture/account7", Some(B), None)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn account_history_is_explicit_and_cannot_cross_account_or_storage_scope() {
        let mut c = db();
        let mut done = receipt(A, "a");
        done.created_at = 1;
        done.targets[0].configuration = ConfigurationState::Existing;
        done.targets[0].connection.state = ConnectionState::Applied;
        done.status = SetupStatus::Settled;
        begin_on(&mut c, BIND, &done).unwrap();
        assert!(
            query_account_on(&c, BIND, 7, "fixture/account7", None, None)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            query_account_on(&c, BIND, 7, "fixture/account7", None, Some(0))
                .unwrap()
                .unwrap()
                .operation_id,
            A
        );
        assert!(query_account_on(&c, BIND, 8, "fixture/account8", Some(A), None).is_err());
        assert!(
            query_account_on(&c, "foreign-storage", 7, "fixture/account7", Some(A), None).is_err()
        );
        assert!(query_account_on(&c, BIND, 7, "fixture/account7", Some(A), Some(0)).is_err());
    }
    #[test]
    fn repeated_start_is_read_only_but_changed_scope_is_rejected() {
        let mut c = db();
        let r = receipt(A, "a");
        assert!(begin_on(&mut c, BIND, &r).unwrap().1);
        assert!(!begin_on(&mut c, BIND, &r).unwrap().1);
        assert!(begin_on(&mut c, BIND, &receipt(A, "b")).is_err());
    }
    #[test]
    fn attempt_is_durable_before_post_and_unknown_never_authorizes_a_repeat() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        key_event_on(&mut c, BIND, A, "a", KeyEvent::BeforeCreate, 2).unwrap();
        assert_eq!(
            read_on(&c, A).unwrap().unwrap().groups[0].state,
            KeyState::Requesting
        );
        key_event_on(&mut c, BIND, A, "a", KeyEvent::OutcomeUnknown, 3).unwrap();
        assert_eq!(
            read_on(&c, A).unwrap().unwrap().groups[0].state,
            KeyState::Unknown
        );
        assert!(key_event_on(&mut c, BIND, A, "a", KeyEvent::BeforeCreate, 4).is_err());
        begin_on(&mut c, BIND, &receipt(B, "a")).unwrap();
        assert!(key_event_on(&mut c, BIND, B, "a", KeyEvent::BeforeCreate, 5).is_err());
        // A different stable ID with the same display name is a different group.
        let other = uuid::Uuid::new_v4().to_string();
        begin_on(&mut c, BIND, &receipt(&other, "b")).unwrap();
        assert!(key_event_on(&mut c, BIND, &other, "b", KeyEvent::BeforeCreate, 6).is_ok());
    }
    #[test]
    fn created_fact_survives_local_failure_and_later_observation() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        key_event_on(&mut c, BIND, A, "a", KeyEvent::BeforeCreate, 2).unwrap();
        key_event_on(
            &mut c,
            BIND,
            A,
            "a",
            KeyEvent::Created(Some("key-1".into())),
            3,
        )
        .unwrap();
        let r = local_fact_on(&mut c, BIND, A, "codex", ConfigurationState::Failed, 4).unwrap();
        assert_eq!(r.created_key_count, 1);
        assert_eq!(r.groups[0].origin, Some(KeyOrigin::Created));
        assert_eq!(r.targets[0].configuration, ConfigurationState::Failed);
        let r = key_event_on(&mut c, BIND, A, "a", KeyEvent::Observed("key-1".into()), 5).unwrap();
        assert_eq!(r.groups[0].origin, Some(KeyOrigin::Created));
        assert_eq!(r.created_key_count, 1);
        assert!(key_event_on(&mut c, BIND, A, "a", KeyEvent::BeforeCreate, 6).is_err());
    }
    #[test]
    fn existing_and_prepared_local_facts_cannot_be_downgraded_to_no_write() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        let r = local_fact_on(&mut c, BIND, A, "codex", ConfigurationState::Existing, 2).unwrap();
        assert_eq!(r.groups[0].state, KeyState::NotStarted);
        let r = local_fact_on(&mut c, BIND, A, "codex", ConfigurationState::Failed, 3).unwrap();
        assert_eq!(r.targets[0].configuration, ConfigurationState::Existing);
        assert_eq!(r.status, SetupStatus::Settled);
    }
    #[test]
    fn foreign_storage_binding_is_never_execution_authority() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        assert!(key_event_on(
            &mut c,
            "restored/generation",
            A,
            "a",
            KeyEvent::BeforeCreate,
            2
        )
        .is_err());
        assert!(begin_on(&mut c, "restored/generation", &receipt(A, "a")).is_err());
    }
    #[test]
    fn journal_failure_prevents_the_external_write() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        c.execute_batch("CREATE TRIGGER deny_attempt BEFORE UPDATE ON relay_setup_operations BEGIN SELECT RAISE(ABORT,'fixture journal failure'); END;").unwrap();
        let mut posted = false;
        let attempt =
            key_event_on(&mut c, BIND, A, "a", KeyEvent::BeforeCreate, 2).map(|_| posted = true);
        assert!(attempt.is_err());
        assert!(!posted);
    }
    #[test]
    fn reused_or_observed_keys_are_not_attributed_as_new_creates() {
        let mut c = db();
        begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
        let r = key_event_on(&mut c, BIND, A, "a", KeyEvent::Reused("key-1".into()), 2).unwrap();
        assert_eq!(r.groups[0].state, KeyState::Ready);
        assert_eq!(r.groups[0].origin, Some(KeyOrigin::Reused));
        assert_eq!(r.created_key_count, 0);
    }
    #[test]
    fn restore_cannot_roll_back_an_unknown_attempt_or_import_foreign_authority() {
        let mut current = db();
        let mut incoming = db();
        begin_on(&mut current, BIND, &receipt(A, "a")).unwrap();
        key_event_on(&mut current, BIND, A, "a", KeyEvent::BeforeCreate, 2).unwrap();
        key_event_on(&mut current, BIND, A, "a", KeyEvent::OutcomeUnknown, 3).unwrap();
        begin_on(&mut incoming, BIND, &receipt(A, "a")).unwrap();
        begin_on(&mut incoming, "foreign-store", &receipt(B, "b")).unwrap();
        preserve_local(&current, &incoming).unwrap();
        assert_eq!(
            read_on(&incoming, A).unwrap(),
            read_on(&current, A).unwrap()
        );
        assert!(read_on(&incoming, B).unwrap().is_none());
    }
    #[test]
    fn reopening_a_database_preserves_uncertainty_and_blocks_a_new_post() {
        let path =
            std::env::temp_dir().join(format!("relay-journal-{}.sqlite", uuid::Uuid::new_v4()));
        {
            let mut c = Connection::open(&path).unwrap();
            create_schema(&c).unwrap();
            begin_on(&mut c, BIND, &receipt(A, "a")).unwrap();
            key_event_on(&mut c, BIND, A, "a", KeyEvent::BeforeCreate, 2).unwrap();
        }
        {
            let mut reopened = Connection::open(&path).unwrap();
            // Interrupted requesting is uncertain, never a fresh notStarted.
            key_event_on(&mut reopened, BIND, A, "a", KeyEvent::OutcomeUnknown, 3).unwrap();
            assert!(key_event_on(&mut reopened, BIND, A, "a", KeyEvent::BeforeCreate, 4).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }
}
