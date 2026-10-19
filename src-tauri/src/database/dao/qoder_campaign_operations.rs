//! One latest non-secret operation receipt per trusted subject/region.
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
const CREATE: &str = "CREATE TABLE IF NOT EXISTS qoder_campaign_operations (
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
pub(crate) fn create_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(CREATE)?;
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
pub(crate) fn validate_schema(conn: &Connection) -> Result<(), AppError> {
    let table: String = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
        [TABLE],
        |r| r.get(0),
    )?;
    if normalized(&table) != normalized(CREATE) {
        return Err(invalid());
    }
    let indices: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_index_list('qoder_campaign_operations')",
        [],
        |r| r.get(0),
    )?;
    let triggers:i64=conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND (tbl_name=?1 OR instr(lower(coalesce(sql,'')),?1)>0)",[TABLE],|r|r.get(0))?;
    let foreign: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_foreign_key_list('qoder_campaign_operations')",
        [],
        |r| r.get(0),
    )?;
    if indices != 2 || triggers != 0 || foreign != 0 {
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
    key: String,
    start: i64,
    end: i64,
    generation: String,
    result: String,
}
pub(crate) fn read_rows(conn: &Connection) -> Result<Vec<Row>, AppError> {
    validate_schema(conn)?;
    let mut stmt=conn.prepare("SELECT attempt_id,subject,region,campaign_id,campaign_key,window_start,window_end,credential_generation,result FROM qoder_campaign_operations ORDER BY attempt_id LIMIT 4097")?;
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
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > 4096 {
        return Err(invalid());
    }
    for r in &rows {
        if [&r.attempt, &r.subject, &r.campaign, &r.key, &r.generation]
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 512 || s.chars().any(char::is_control))
            || r.region != "international"
            || r.start < 0
            || r.end <= r.start
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
            r.key.clone(),
            r.start,
            r.end,
        )
        .ok_or_else(invalid)?;
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
    tx.execute("DELETE FROM qoder_campaign_operations", [])?;
    for r in &rows {
        tx.execute(
            "INSERT INTO qoder_campaign_operations VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                r.attempt,
                r.subject,
                r.region,
                r.campaign,
                r.key,
                r.start,
                r.end,
                r.generation,
                r.result
            ],
        )?;
    }
    if read_rows(&tx)? != rows {
        return Err(invalid());
    }
    tx.commit()?;
    Ok(())
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
        "claimed_delta_unknown" => Ok(Outcome::ClaimedDeltaUnknown),
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
            let binding =
                BoundCampaign::from_operation(identity.clone(), r.campaign, r.key, r.start, r.end)
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
