//! One non-secret anti-replay owner: one latest Intl receipt per subject/region,
//! and one CN receipt per subject/region/campaign. Optional CN facts stay absent.
//! It survives account deletion and is replaced only by a later non-overlapping
//! authenticated window. Time alone never erases the anti-replay lower bound.
//! Explicit whole-database erasure removes it; no hidden recovery copy exists.
//! Mutation functions require the caller's existing Provider SQLite transaction.
use crate::{
    error::AppError,
    services::provider::qoder_daily::{
        contract::{BoundCampaign, Identity, Outcome, Region},
        Operation,
    },
};
use rusqlite::{params, Connection};
const TABLE: &str = "qoder_campaign_operations";
const LEGACY_CREATE: &str = "CREATE TABLE IF NOT EXISTS qoder_campaign_operations (
 attempt_id TEXT PRIMARY KEY NOT NULL CHECK(length(attempt_id)>0),
 subject TEXT NOT NULL CHECK(length(subject)>0),
 region TEXT NOT NULL CHECK(region='international'),
 campaign_id TEXT NOT NULL CHECK(length(campaign_id)>0),
 campaign_key TEXT NOT NULL CHECK(length(campaign_key)>0),
 window_start INTEGER NOT NULL CHECK(window_start>=0),
 window_end INTEGER NOT NULL CHECK(window_end>window_start),
 credential_generation TEXT NOT NULL CHECK(length(credential_generation)>0),
 result TEXT NOT NULL CHECK(result IN ('unknown','claimed','replayed','claimed_delta_unknown')),
 UNIQUE(subject,region));";
const CREATE: &str = "CREATE TABLE IF NOT EXISTS qoder_campaign_operations (
 attempt_id TEXT PRIMARY KEY NOT NULL CHECK(length(attempt_id)>0),
 subject TEXT NOT NULL CHECK(length(subject)>0),
 region TEXT NOT NULL CHECK(region IN ('international','china')),
 campaign_id TEXT NOT NULL CHECK(length(campaign_id)>0),
 campaign_key TEXT CHECK(campaign_key IS NULL OR length(campaign_key)>0),
 window_start INTEGER CHECK(window_start IS NULL OR window_start>=0),
 window_end INTEGER CHECK(window_end IS NULL OR window_end>=0),
 credential_generation TEXT NOT NULL CHECK(length(credential_generation)>0),
 result TEXT NOT NULL CHECK(result IN ('unknown','claimed','replayed','claimed_delta_unknown','cn_claimed_observed','cn_reconciled')),
 provider_id TEXT CHECK(provider_id IS NULL OR length(provider_id)>0),
 CHECK(window_start IS NULL OR window_end IS NULL OR window_end>window_start),
 CHECK((region='international' AND campaign_key IS NOT NULL AND window_start IS NOT NULL AND window_end IS NOT NULL AND provider_id IS NULL AND result IN ('unknown','claimed','replayed','claimed_delta_unknown')) OR
       (region='china' AND provider_id IS NOT NULL AND result IN ('unknown','claimed_delta_unknown','cn_claimed_observed','cn_reconciled'))));";
const INTL_INDEX: &str = "CREATE UNIQUE INDEX qoder_operations_intl_subject ON qoder_campaign_operations(subject,region) WHERE region='international'";
const CN_INDEX: &str = "CREATE UNIQUE INDEX qoder_operations_cn_campaign ON qoder_campaign_operations(subject,region,campaign_id) WHERE region='china'";

pub(crate) fn create_schema(conn: &Connection) -> Result<(), AppError> {
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [TABLE],
        |r| r.get(0),
    )?;
    if exists == 0 {
        conn.execute_batch(CREATE)?;
        conn.execute_batch(INTL_INDEX)?;
        conn.execute_batch(CN_INDEX)?;
    }
    validate_schema(conn)
}
fn normalized(value: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    for c in value.chars() {
        if c == '\'' {
            quoted = !quoted;
            out.push(c);
        } else if quoted {
            out.push(c);
        } else if !c.is_whitespace() {
            out.extend(c.to_lowercase());
        }
    }
    out.replace("ifnotexists", "")
        .trim_end_matches(';')
        .to_string()
}
/// Recognize only the exact published schemas. Legacy shapes are read-only
/// before v29; an installed v29 database cannot downgrade to the old shape.
fn legacy_schema(conn: &Connection) -> Result<bool, AppError> {
    let table: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
        [TABLE],
        |r| r.get(0),
    )?;
    if normalized(&table) == normalized(CREATE) {
        return Ok(false);
    }
    if normalized(&table) == normalized(LEGACY_CREATE)
        && crate::database::loongport_schema::read_stored_version(conn)? < 29
    {
        return Ok(true);
    }
    Err(invalid())
}
pub(crate) fn validate_schema(conn: &Connection) -> Result<(), AppError> {
    let legacy = legacy_schema(conn)?;
    let indices: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_index_list('qoder_campaign_operations')",
        [],
        |r| r.get(0),
    )?;
    let triggers: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND (tbl_name=?1 OR instr(lower(coalesce(sql,'')),?1)>0)", [TABLE], |r|r.get(0))?;
    let foreign: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_foreign_key_list('qoder_campaign_operations')",
        [],
        |r| r.get(0),
    )?;
    if indices != if legacy { 2 } else { 3 } || triggers != 0 || foreign != 0 {
        return Err(invalid());
    }
    if !legacy {
        for (name, expected) in [
            ("qoder_operations_intl_subject", INTL_INDEX),
            ("qoder_operations_cn_campaign", CN_INDEX),
        ] {
            let sql: String = conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND tbl_name=?1 AND name=?2",
                params![TABLE, name],
                |r| r.get(0),
            )?;
            if normalized(&sql) != normalized(expected) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
/// Called by the v28 -> v29 owner inside its migration savepoint. No receipt is
/// inferred or dropped; already-current fresh databases are validated only.
pub(crate) fn migrate_cn_schema(conn: &Connection) -> Result<(), AppError> {
    let before = read_rows(conn)?;
    if !legacy_schema(conn)? {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE qoder_campaign_operations RENAME TO qoder_campaign_operations_v27",
    )?;
    create_schema(conn)?;
    conn.execute_batch("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result) SELECT attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result FROM qoder_campaign_operations_v27; DROP TABLE qoder_campaign_operations_v27;")?;
    if read_rows(conn)? != before {
        return Err(invalid());
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    attempt: String,
    subject: String,
    region: String,
    campaign: String,
    key: Option<String>,
    start: Option<i64>,
    end: Option<i64>,
    generation: String,
    result: String,
    provider: Option<String>,
}
pub(crate) fn read_rows(conn: &Connection) -> Result<Vec<Row>, AppError> {
    validate_schema(conn)?;
    let provider = if legacy_schema(conn)? {
        "NULL"
    } else {
        "provider_id"
    };
    let mut stmt=conn.prepare(&format!("SELECT attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result,{provider} FROM qoder_campaign_operations ORDER BY attempt_id LIMIT 4097"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Row {
                attempt: r.get(0)?,
                subject: r.get(1)?,
                region: r.get(2)?,
                campaign: r.get(3)?,
                key: r.get(4)?,
                start: r.get(5)?,
                end: r.get(6)?,
                generation: r.get(7)?,
                result: r.get(8)?,
                provider: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > 4096 {
        return Err(invalid());
    }
    for r in &rows {
        if [&r.attempt, &r.subject, &r.campaign, &r.generation]
            .into_iter()
            .any(|s| !valid_text(s))
            || r.key.as_deref().is_some_and(|s| !valid_text(s))
            || r.provider.as_deref().is_some_and(|s| !valid_text(s))
        {
            return Err(invalid());
        }
        match r.region.as_str() {
            "international" => {
                if r.provider.is_some()
                    || matches!(r.result.as_str(), "cn_claimed_observed" | "cn_reconciled")
                {
                    return Err(invalid());
                }
                BoundCampaign::from_operation(
                    Identity {
                        provider_id: "validated-row".into(),
                        uid: r.subject.clone(),
                        region: Region::International,
                    },
                    r.campaign.clone(),
                    r.key.clone().ok_or_else(invalid)?,
                    r.start.ok_or_else(invalid)?,
                    r.end.ok_or_else(invalid)?,
                )
                .ok_or_else(invalid)?;
            }
            "china" => {
                if r.provider.is_none()
                    || !matches!(
                        r.result.as_str(),
                        "unknown"
                            | "claimed_delta_unknown"
                            | "cn_claimed_observed"
                            | "cn_reconciled"
                    )
                    || !(CnTarget {
                        campaign_id: r.campaign.clone(),
                        campaign_key: r.key.clone(),
                        start_at: r.start,
                        end_at: r.end,
                    })
                    .valid()
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        result_value(&r.result)?;
    }
    Ok(rows)
}
/// Called only by existing unpublished staging/restore owners while live DB is locked.
/// Keep local operation facts; foreign account receipts never become local evidence.
pub(crate) fn preserve_local(live: &Connection, target: &Connection) -> Result<(), AppError> {
    let rows = read_rows(live)?;
    if crate::database::loongport_schema::read_stored_version(target)? >= 26 {
        validate_schema(target)?;
    } else {
        create_schema(target)?;
    }
    let tx = target.unchecked_transaction()?;
    migrate_cn_schema(&tx)?;
    tx.execute("DELETE FROM qoder_campaign_operations", [])?;
    for r in &rows {
        tx.execute(
            "INSERT INTO qoder_campaign_operations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                r.attempt,
                r.subject,
                r.region,
                r.campaign,
                r.key,
                r.start,
                r.end,
                r.generation,
                r.result,
                r.provider
            ],
        )?;
    }
    if read_rows(&tx)? != rows {
        return Err(invalid());
    }
    tx.commit()?;
    Ok(())
}
fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn valid_cn_target_text(value: &str) -> bool {
    valid_text(value)
        && value.len() <= 256
        && !value
            .chars()
            .any(|c| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CnTarget {
    pub(crate) campaign_id: String,
    pub(crate) campaign_key: Option<String>,
    pub(crate) start_at: Option<i64>,
    pub(crate) end_at: Option<i64>,
}
impl CnTarget {
    pub(crate) fn valid(&self) -> bool {
        valid_cn_target_text(&self.campaign_id)
            && !matches!(self.campaign_id.as_str(), "." | "..")
            && self
                .campaign_key
                .as_deref()
                .is_none_or(valid_cn_target_text)
            && self.start_at.is_none_or(|v| v >= 0)
            && self.end_at.is_none_or(|v| v >= 0)
            && !matches!((self.start_at, self.end_at), (Some(start), Some(end)) if end <= start)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CnOperation {
    pub(crate) id: String,
    pub(crate) generation: String,
    pub(crate) identity: Identity,
    pub(crate) target: CnTarget,
    pub(crate) outcome: Outcome,
    /// True only after a confirmed POST receipt, never a GET reconciliation.
    pub(crate) receipt_confirmed: bool,
    /// An independent later listing observed this receipt-confirmed target CLAIMED.
    pub(crate) listing_claimed: bool,
}
fn valid_cn(operation: &CnOperation) -> bool {
    operation.identity.region == Region::China
        && [
            &operation.id,
            &operation.generation,
            &operation.identity.provider_id,
            &operation.identity.uid,
        ]
        .into_iter()
        .all(|s| valid_text(s))
        && operation.target.valid()
        && match operation.outcome {
            Outcome::Unknown => !operation.receipt_confirmed && !operation.listing_claimed,
            Outcome::ClaimedDeltaUnknown => {
                !operation.listing_claimed || operation.receipt_confirmed
            }
            _ => false,
        }
}
/// The UID owns the anti-replay row. Read through duplicate providers without
/// rebinding the original dispatch provider or credential generation.
pub(crate) fn read_cn_on(
    conn: &Connection,
    identity: &Identity,
) -> Result<Vec<CnOperation>, AppError> {
    if identity.region != Region::China
        || !valid_text(&identity.uid)
        || !valid_text(&identity.provider_id)
    {
        return Err(invalid());
    }
    read_rows(conn)?
        .into_iter()
        .filter(|r| r.subject == identity.uid && r.region == "china")
        .map(|r| {
            Ok(CnOperation {
                id: r.attempt,
                generation: r.generation,
                identity: Identity {
                    provider_id: r.provider.ok_or_else(invalid)?,
                    uid: r.subject,
                    region: Region::China,
                },
                target: CnTarget {
                    campaign_id: r.campaign,
                    campaign_key: r.key,
                    start_at: r.start,
                    end_at: r.end,
                },
                outcome: result_value(&r.result)?,
                receipt_confirmed: matches!(
                    r.result.as_str(),
                    "claimed_delta_unknown" | "cn_claimed_observed"
                ),
                listing_claimed: r.result == "cn_claimed_observed",
            })
        })
        .collect()
}
/// Caller holds the Provider transaction so source admission and this durable
/// pre-POST receipt commit together. Unknown never authorizes a second POST.
pub(crate) fn insert_cn_unknown_on(
    conn: &Connection,
    operation: &CnOperation,
) -> Result<Option<CnOperation>, AppError> {
    if conn.is_autocommit() || !valid_cn(operation) || operation.outcome != Outcome::Unknown {
        return Err(invalid());
    }
    let previous = read_cn_on(conn, &operation.identity)?
        .into_iter()
        .find(|r| r.target.campaign_id == operation.target.campaign_id);
    if let Some(old) = &previous {
        let proven_advance = matches!((old.target.start_at,old.target.end_at,operation.target.start_at,operation.target.end_at),
            (Some(_),Some(old_end),Some(new_start),Some(_)) if new_start >= old_end);
        // This observed transition is an authorization based on service listing
        // state, not proof of a new server round or an exactly-once guarantee.
        let observed_transition = old.receipt_confirmed
            && old.listing_claimed
            && old.identity == operation.identity
            && old.generation == operation.generation
            && old.target == operation.target;
        if old.outcome == Outcome::Unknown || !(proven_advance || observed_transition) {
            return Err(AppError::Config("qoder.window_not_advanced".into()));
        }
        conn.execute("DELETE FROM qoder_campaign_operations WHERE attempt_id=?1 AND subject=?2 AND region='china' AND campaign_id=?3 AND credential_generation=?4 AND provider_id=?5",params![old.id,old.identity.uid,old.target.campaign_id,old.generation,old.identity.provider_id])?;
    }
    let t = &operation.target;
    conn.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result,provider_id) VALUES(?1,?2,'china',?3,?4,?5,?6,?7,'unknown',?8)",params![operation.id,operation.identity.uid,t.campaign_id,t.campaign_key,t.start_at,t.end_at,operation.generation,operation.identity.provider_id])?;
    read_rows(conn)?;
    Ok(previous)
}
/// Only an exact original dispatch binding can resolve a CN Unknown. A success
/// records no fabricated amount; repeated resolution cannot downgrade evidence.
pub(crate) fn update_cn_result_on(
    conn: &Connection,
    operation: &CnOperation,
    outcome: Outcome,
) -> Result<(), AppError> {
    if conn.is_autocommit()
        || !valid_cn(operation)
        || !matches!(outcome, Outcome::Unknown | Outcome::ClaimedDeltaUnknown)
    {
        return Err(invalid());
    }
    read_rows(conn)?;
    let t = &operation.target;
    let changed=conn.execute("UPDATE qoder_campaign_operations SET result=?1 WHERE attempt_id=?2 AND subject=?3 AND region='china' AND campaign_id=?4 AND campaign_key IS ?5 AND window_start IS ?6 AND window_end IS ?7 AND credential_generation=?8 AND provider_id=?9 AND (result='unknown' OR (?1='claimed_delta_unknown' AND result='cn_reconciled'))",params![result_name(outcome),operation.id,operation.identity.uid,t.campaign_id,t.campaign_key,t.start_at,t.end_at,operation.generation,operation.identity.provider_id])?;
    if changed != 1
        && !read_cn_on(conn, &operation.identity)?.into_iter().any(|r| {
            r.id == operation.id
                && r.generation == operation.generation
                && r.identity == operation.identity
                && r.target == operation.target
                && r.outcome == Outcome::ClaimedDeltaUnknown
        })
    {
        return Err(invalid());
    }
    Ok(())
}
/// A GET can close an ambiguous intent for display, but cannot grant the
/// receipt-confirmed marker used for an unwindowed same-target transition.
pub(crate) fn reconcile_cn_result_on(
    conn: &Connection,
    operation: &CnOperation,
) -> Result<(), AppError> {
    if conn.is_autocommit() || !valid_cn(operation) {
        return Err(invalid());
    }
    read_rows(conn)?;
    let t = &operation.target;
    let changed=conn.execute("UPDATE qoder_campaign_operations SET result='cn_reconciled' WHERE attempt_id=?1 AND subject=?2 AND region='china' AND campaign_id=?3 AND campaign_key IS ?4 AND window_start IS ?5 AND window_end IS ?6 AND credential_generation=?7 AND provider_id=?8 AND result='unknown'",params![operation.id,operation.identity.uid,t.campaign_id,t.campaign_key,t.start_at,t.end_at,operation.generation,operation.identity.provider_id])?;
    if changed != 1
        && !read_cn_on(conn, &operation.identity)?
            .into_iter()
            .any(|r| same_cn_binding(&r, operation) && r.outcome == Outcome::ClaimedDeltaUnknown)
    {
        return Err(invalid());
    }
    Ok(())
}
/// Caller observed CLAIMED in an independent GET after the POST receipt was
/// already durable. An Unknown or GET-reconciled operation cannot acquire this
/// marker, even when its caller supplies forged receipt-confirmed flags.
pub(crate) fn observe_cn_claimed_on(
    conn: &Connection,
    operation: &CnOperation,
) -> Result<(), AppError> {
    if conn.is_autocommit()
        || !valid_cn(operation)
        || operation.outcome != Outcome::ClaimedDeltaUnknown
        || !operation.receipt_confirmed
    {
        return Err(invalid());
    }
    let current = read_cn_on(conn, &operation.identity)?
        .into_iter()
        .find(|r| same_cn_binding(r, operation))
        .ok_or_else(invalid)?;
    if !current.receipt_confirmed {
        return Err(invalid());
    }
    let t = &operation.target;
    let changed=conn.execute("UPDATE qoder_campaign_operations SET result='cn_claimed_observed' WHERE attempt_id=?1 AND subject=?2 AND region='china' AND campaign_id=?3 AND campaign_key IS ?4 AND window_start IS ?5 AND window_end IS ?6 AND credential_generation=?7 AND provider_id=?8 AND result='claimed_delta_unknown'",params![operation.id,operation.identity.uid,t.campaign_id,t.campaign_key,t.start_at,t.end_at,operation.generation,operation.identity.provider_id])?;
    if changed != 1 && !current.listing_claimed {
        return Err(invalid());
    }
    Ok(())
}
fn same_cn_binding(left: &CnOperation, right: &CnOperation) -> bool {
    left.id == right.id
        && left.generation == right.generation
        && left.identity == right.identity
        && left.target == right.target
}
fn invalid() -> AppError {
    AppError::Config("qoder.operation_receipts_invalid".into())
}
fn result_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::NewlyClaimed => "claimed",
        Outcome::AlreadyClaimed => "replayed",
        Outcome::ClaimedDeltaUnknown => "claimed_delta_unknown",
        _ => "unknown",
    }
}
fn result_value(value: &str) -> Result<Outcome, AppError> {
    match value {
        "unknown" => Ok(Outcome::Unknown),
        "claimed" => Ok(Outcome::NewlyClaimed),
        "replayed" => Ok(Outcome::AlreadyClaimed),
        "claimed_delta_unknown" | "cn_claimed_observed" | "cn_reconciled" => {
            Ok(Outcome::ClaimedDeltaUnknown)
        }
        _ => Err(invalid()),
    }
}
pub(crate) fn read_on(conn: &Connection, identity: &Identity) -> Result<Vec<Operation>, AppError> {
    if identity.region != Region::International || identity.uid.trim().is_empty() {
        return Err(invalid());
    }
    read_rows(conn)?
        .into_iter()
        .filter(|r| r.subject == identity.uid && r.region == "international")
        .map(|r| {
            let binding = BoundCampaign::from_operation(
                identity.clone(),
                r.campaign,
                r.key.ok_or_else(invalid)?,
                r.start.ok_or_else(invalid)?,
                r.end.ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?;
            Ok(Operation {
                id: r.attempt,
                generation: r.generation,
                binding,
                outcome: result_value(&r.result)?,
            })
        })
        .collect()
}
/// The caller owns the Provider transaction. Replacement and pending detachment
/// must commit together. Unknown is GET-only until a receipt resolves it.
pub(crate) fn insert_unknown_on(
    conn: &Connection,
    operation: &Operation,
) -> Result<Option<Operation>, AppError> {
    let b = &operation.binding;
    if !b.valid_persisted() || operation.outcome != Outcome::Unknown {
        return Err(invalid());
    }
    let previous = read_on(conn, b.identity())?.into_iter().next();
    if let Some(old) = &previous {
        if old.outcome == Outcome::Unknown || b.start_at() < old.binding.end_at() {
            return Err(AppError::Config("qoder.window_not_advanced".into()));
        }
        conn.execute("DELETE FROM qoder_campaign_operations WHERE subject=?1 AND region='international' AND attempt_id=?2 AND credential_generation=?3",params![b.identity().uid,old.id,old.generation])?;
    }
    conn.execute("INSERT INTO qoder_campaign_operations(attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result) VALUES(?1,?2,'international',?3,?4,?5,?6,?7,'unknown')",params![operation.id,b.identity().uid,b.target().campaign_id,b.target().campaign_key,b.start_at(),b.end_at(),operation.generation])?;
    // Refuse capacity/field-limit violations before the caller may commit.
    read_rows(conn)?;
    Ok(previous)
}
pub(crate) fn update_result_on(
    conn: &Connection,
    operation: &Operation,
    outcome: Outcome,
) -> Result<(), AppError> {
    let b = &operation.binding;
    let changed=conn.execute("UPDATE qoder_campaign_operations SET result=?1 WHERE attempt_id=?2 AND credential_generation=?3 AND subject=?4 AND region='international' AND campaign_id=?5 AND campaign_key=?6 AND window_start=?7 AND window_end=?8 AND result='unknown'",params![result_name(outcome),operation.id,operation.generation,b.identity().uid,b.target().campaign_id,b.target().campaign_key,b.start_at(),b.end_at()])?;
    if changed != 1 {
        let current = read_on(conn, b.identity())?;
        if !current.iter().any(|r| {
            r.id == operation.id
                && r.generation == operation.generation
                && r.binding == operation.binding
                && r.outcome != Outcome::Unknown
        }) {
            return Err(invalid());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "qoder_campaign_operations_tests.rs"]
mod tests;
