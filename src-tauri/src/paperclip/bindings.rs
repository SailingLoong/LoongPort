//! Device-local metadata inside the original Database schema and restore owner.
//! This is not an account, credential, Vault, or alternate database owner.
use super::contract;
use crate::error::AppError;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
pub(crate) const TABLE: &str = "paperclip_run_bindings";
const DDL:&str="CREATE TABLE IF NOT EXISTS paperclip_run_bindings(owner TEXT NOT NULL,app TEXT NOT NULL,company TEXT NOT NULL,run TEXT NOT NULL,scope TEXT NOT NULL,binding TEXT NOT NULL,phase TEXT NOT NULL CHECK(phase IN ('bound','accepted','completed','failed','unknown')),receipt TEXT,PRIMARY KEY(owner,app,company,run))";
fn denied() -> AppError {
    AppError::Config("paperclip.ledger_unavailable".into())
}
pub(crate) fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    pub company_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub run_id: String,
    pub resource_id: String,
    pub owner_id: String,
    pub snapshot_revision: String,
}
impl Scope {
    pub(crate) fn valid(&self) -> bool {
        [
            &self.company_id,
            &self.task_id,
            &self.agent_id,
            &self.run_id,
            &self.resource_id,
            &self.owner_id,
            &self.snapshot_revision,
        ]
        .into_iter()
        .all(|s| id(s))
    }
}
#[allow(clippy::too_many_arguments)] // Exact persisted eight-column row validator.
pub(crate) fn validate_row(
    owner: &str,
    app: &str,
    company: &str,
    run: &str,
    scope: &str,
    binding: &str,
    phase: &str,
    receipt: Option<&str>,
) -> Result<(), AppError> {
    if scope.len() > 2048 || binding.len() > 32768 || receipt.is_some_and(|s| s.len() > 49152) {
        return Err(denied());
    }
    let s: Scope = serde_json::from_str(scope).map_err(|_| denied())?;
    if s.owner_id != owner || s.company_id != company || s.run_id != run {
        return Err(denied());
    }
    if !s.valid() || !id(app) {
        return Err(denied());
    }
    let actual: Value = serde_json::from_str(binding).map_err(|_| denied())?;
    contract::validate_binding(&actual, &s, app)?;
    match (phase, receipt) {
        ("bound" | "accepted" | "unknown", None) => Ok(()),
        ("completed" | "failed" | "unknown", Some(raw)) => {
            let r: Value = serde_json::from_str(raw).map_err(|_| denied())?;
            if r["status"] != phase {
                return Err(denied());
            }
            contract::validate_receipt(&actual, &r)
        }
        _ => Err(denied()),
    }
}
pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), AppError> {
    let stored = crate::database::loongport_schema::read_stored_version(conn)?;
    if stored >= 25 {
        return validate_schema(conn);
    }
    conn.execute_batch(DDL).map_err(|_| denied())?;
    validate_schema(conn)
}
pub(crate) fn validate_schema(conn: &Connection) -> Result<(), AppError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(paperclip_run_bindings)")
        .map_err(|_| denied())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(|_| denied())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| denied())?;
    let expected = [
        ("owner", 1, 1),
        ("app", 1, 2),
        ("company", 1, 3),
        ("run", 1, 4),
        ("scope", 1, 0),
        ("binding", 1, 0),
        ("phase", 1, 0),
        ("receipt", 0, 0),
    ];
    if rows.len() != expected.len()
        || !rows
            .iter()
            .zip(expected)
            .all(|((name, ty, nn, pk), (e, n, p))| {
                name == e && ty == "TEXT" && *nn == n && *pk == p
            })
    {
        return Err(denied());
    }
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            [TABLE],
            |r| r.get(0),
        )
        .map_err(|_| denied())?;
    let normalized = |value: &str| {
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
        out.replacen("createtableifnotexists", "createtable", 1)
    };
    if normalized(&sql) != normalized(DDL) {
        return Err(denied());
    }
    let indices: Vec<(String, String, i64)> = conn
        .prepare("SELECT name,origin,partial FROM pragma_index_list('paperclip_run_bindings')")
        .map_err(|_| denied())?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|_| denied())?
        .collect::<Result<_, _>>()
        .map_err(|_| denied())?;
    if indices.len() != 1 || indices[0].1 != "pk" || indices[0].2 != 0 {
        return Err(denied());
    }
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
        .map_err(|_| denied())?
        .query_map([&indices[0].0], |r| r.get(0))
        .map_err(|_| denied())?
        .collect::<Result<_, _>>()
        .map_err(|_| denied())?;
    if columns != ["owner", "app", "company", "run"] {
        return Err(denied());
    }
    let triggers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND (tbl_name=?1 OR instr(lower(coalesce(sql,'')),?1)>0)",
            [TABLE],
            |r| r.get(0),
        )
        .map_err(|_| denied())?;
    let foreign: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_list('paperclip_run_bindings')",
            [],
            |r| r.get(0),
        )
        .map_err(|_| denied())?;
    if triggers != 0 || foreign != 0 {
        return Err(denied());
    }
    Ok(())
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Row {
    owner: String,
    app: String,
    company: String,
    run: String,
    scope: String,
    binding: String,
    phase: String,
    receipt: Option<String>,
}
pub(crate) fn read_rows(conn: &Connection) -> Result<Vec<Row>, AppError> {
    validate_schema(conn)?;
    let mut stmt=conn.prepare("SELECT owner,app,company,run,scope,binding,phase,receipt FROM paperclip_run_bindings ORDER BY owner,app,company,run LIMIT 1025").map_err(|_|denied())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Row {
                owner: r.get(0)?,
                app: r.get(1)?,
                company: r.get(2)?,
                run: r.get(3)?,
                scope: r.get(4)?,
                binding: r.get(5)?,
                phase: r.get(6)?,
                receipt: r.get(7)?,
            })
        })
        .map_err(|_| denied())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| denied())?;
    if rows.len() > 1024 {
        return Err(denied());
    }
    for r in &rows {
        validate_row(
            &r.owner,
            &r.app,
            &r.company,
            &r.run,
            &r.scope,
            &r.binding,
            &r.phase,
            r.receipt.as_deref(),
        )?;
    }
    Ok(rows)
}
/// Caller holds ORIGINAL live connection admission. Target is unpublished staging.
/// Incoming ledger rows are discarded; only validated live device evidence survives.
pub(crate) fn preserve_local(live: &Connection, target: &Connection) -> Result<(), AppError> {
    let rows = read_rows(live)?;
    ensure_schema(target)?;
    let tx = target.unchecked_transaction().map_err(|_| denied())?;
    tx.execute("DELETE FROM paperclip_run_bindings", [])
        .map_err(|_| denied())?;
    for r in &rows {
        tx.execute(
            "INSERT INTO paperclip_run_bindings VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![r.owner, r.app, r.company, r.run, r.scope, r.binding, r.phase, r.receipt],
        )
        .map_err(|_| denied())?;
    }
    if read_rows(&tx)? != rows {
        return Err(denied());
    }
    tx.commit().map_err(|_| denied())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn example() -> (Scope, Value) {
        let s = Scope {
            company_id: "company".into(),
            task_id: "task".into(),
            agent_id: "agent".into(),
            run_id: "run".into(),
            resource_id: "pool".into(),
            owner_id: "owner".into(),
            snapshot_revision: "revision".into(),
        };
        let task:contract::Task=serde_json::from_value(json!({"version":3,"input":"private synthetic prompt","model":"fixture","maxOutputTokens":100,"timeoutMs":1000,"budget":{"mode":"estimated","currency":"USD","limitMicros":1000}})).unwrap();
        let b = contract::binding(&s, "codex", &task).unwrap();
        (s, b)
    }
    fn insert(c: &Connection, phase: &str, receipt: Option<Value>) {
        let (s, b) = example();
        c.execute(
            "INSERT INTO paperclip_run_bindings VALUES(?1,'codex',?2,?3,?4,?5,?6,?7)",
            params![
                s.owner_id,
                s.company_id,
                s.run_id,
                serde_json::to_string(&s).unwrap(),
                b.to_string(),
                phase,
                receipt.map(|r| r.to_string())
            ],
        )
        .unwrap();
    }
    #[test]
    fn original_database_creates_schema() {
        let db = crate::database::Database::memory().unwrap();
        validate_schema(&db.conn.lock().unwrap()).unwrap();
    }
    #[test]
    fn local_unknown_preserved_and_foreign_rows_discarded() {
        let live = Connection::open_in_memory().unwrap();
        ensure_schema(&live).unwrap();
        insert(&live, "unknown", None);
        let other = Connection::open_in_memory().unwrap();
        ensure_schema(&other).unwrap();
        insert(&other, "failed", Some(json!({"forged":true})));
        preserve_local(&live, &other).unwrap();
        assert_eq!(read_rows(&live).unwrap(), read_rows(&other).unwrap());
    }
    #[test]
    fn corrupt_local_evidence_blocks_restore() {
        let live = Connection::open_in_memory().unwrap();
        let other = Connection::open_in_memory().unwrap();
        assert!(preserve_local(&live, &other).is_err());
        ensure_schema(&live).unwrap();
        insert(&live, "failed", Some(json!({})));
        assert!(preserve_local(&live, &other).is_err());
    }
    #[test]
    fn installed_missing_table_not_recreated() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY,version INTEGER); INSERT INTO loongport_schema_version VALUES(1,25)").unwrap();
        assert!(ensure_schema(&c).is_err());
    }
    #[test]
    fn unsafe_table_shape_rejected() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(&DDL.replace(
            "CHECK(phase IN ('bound','accepted','completed','failed','unknown'))",
            "",
        ))
        .unwrap();
        assert!(validate_schema(&c).is_err());
    }
    #[test]
    fn stored_binding_never_contains_prompt_or_answer() {
        let c = Connection::open_in_memory().unwrap();
        ensure_schema(&c).unwrap();
        insert(&c, "accepted", None);
        let rows = read_rows(&c).unwrap();
        assert!(!rows[0].binding.contains("private synthetic prompt"));
        assert!(!rows[0].binding.contains("input"));
    }
}
