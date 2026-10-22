//! Explicit read-only OpenCode V2 import, using the existing Personal key owner.
//! Contract: anomalyco/opencode packages/core/src/credential/sql.ts and
//! packages/schema/src/credential{,-legacy}.ts (2026-10-22).
//! Never reads auth.json (stale after V2 migration), OAuth access/refresh tokens,
//! other integrations, or writes/changes the official OpenCode login selection.
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(300);
const MAX_ROWS: usize = 100;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub is_active: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub preview_id: String,
    pub entries: Vec<Entry>,
    pub notices: Vec<String>,
}

struct Ticket {
    id: String,
    path: PathBuf,
    fingerprint: Vec<u8>,
    issued: Instant,
}
#[derive(Default)]
pub struct Session {
    pending: Option<Ticket>,
}
// Deliberately not Debug or Serialize. These values never cross the IPC boundary.
pub struct Selected {
    pub key: String,
    pub label: String,
}
struct Source {
    id: String,
    label: String,
    active: Option<i64>,
    kind: String,
    key_value: Option<String>,
    updated: i64,
}
impl Session {
    pub fn preview(
        &mut self,
        path: &Path,
        consent: bool,
        now: Instant,
    ) -> Result<Preview, &'static str> {
        if !consent {
            return Err("consent_required");
        }
        // A fresh read retires the preceding ticket even when the read fails.
        self.pending = None;
        let path = canonical_source(path)?;
        let rows = read_source(&path)?;
        let mut entries = Vec::new();
        let mut notices = Vec::new();
        for row in &rows {
            if let Some(selected) = row.selected() {
                entries.push(Entry {
                    id: entry_id(&row.id),
                    label: selected.label,
                    is_active: row.active == Some(1),
                });
            } else {
                let notice = if row.kind == "oauth" {
                    "oauth_not_imported"
                } else {
                    "unsupported_credential"
                };
                if !notices.iter().any(|s| s == notice) {
                    notices.push(notice.to_string());
                }
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.pending = Some(Ticket {
            id: id.clone(),
            path,
            fingerprint: fingerprint(&rows),
            issued: now,
        });
        Ok(Preview {
            preview_id: id,
            entries,
            notices,
        })
    }

    pub fn cancel(&mut self, id: &str) {
        if self.pending.as_ref().is_some_and(|t| t.id == id) {
            self.pending = None;
        }
    }

    pub fn take(
        &mut self,
        path: &Path,
        id: &str,
        entry: &str,
        consent: bool,
        now: Instant,
    ) -> Result<Selected, &'static str> {
        if !consent {
            return Err("consent_required");
        }
        if self.pending.as_ref().is_none_or(|t| t.id != id) {
            return Err("preview_unavailable");
        }
        // Single-use before any target write, including on failure.
        let ticket = self.pending.take().ok_or("preview_unavailable")?;
        if now.saturating_duration_since(ticket.issued) >= TTL {
            return Err("preview_expired");
        }
        if canonical_source(path)? != ticket.path {
            return Err("source_changed");
        }
        let rows = read_source(&ticket.path)?;
        if fingerprint(&rows) != ticket.fingerprint {
            return Err("source_changed");
        }
        rows.iter()
            .find(|row| entry_id(&row.id) == entry)
            .and_then(Source::selected)
            .ok_or("unsupported_credential")
    }
}
impl Source {
    fn selected(&self) -> Option<Selected> {
        if self.kind != "key" {
            return None;
        }
        let value: Value = serde_json::from_str(self.key_value.as_deref()?).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("key") {
            return None;
        }
        let key = value.get("key")?.as_str()?;
        super::opencode_personal::identity(key).ok()?;
        // V2.1 moved configuration answers into metadata; legacy configuration wins.
        let setting = |name: &str| {
            value
                .get("configuration")
                .and_then(|v| v.get(name))
                .or_else(|| value.get("metadata").and_then(|v| v.get(name)))
        };
        if let Some(server) = setting("server") {
            if !server.is_null()
                && !matches!(
                    server.as_str(),
                    Some("https://opencode.ai/console" | "https://opencode.ai/console/")
                )
            {
                return None;
            }
        }
        // The Personal key owner does not carry an organization-scoped session.
        if setting("orgID").is_some_and(|v| !v.is_null()) {
            return None;
        }
        let label = self.label.trim();
        let label = if label.is_empty()
            || label.contains(key)
            || label.contains("oc_sk_")
            || label.chars().count() > 100
            || label.chars().any(char::is_control)
        {
            "OpenCode · 本机 API key".to_string()
        } else {
            label.to_string()
        };
        Some(Selected {
            key: key.to_string(),
            label,
        })
    }
}
fn canonical_source(path: &Path) -> Result<PathBuf, &'static str> {
    let path = path.canonicalize().map_err(|_| "source_unavailable")?;
    if !path.is_file() {
        return Err("source_unavailable");
    }
    Ok(path)
}
fn sqlite_error(error: rusqlite::Error) -> &'static str {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            "source_locked"
        }
        _ => "unsupported_schema",
    }
}
fn read_source(path: &Path) -> Result<Vec<Source>, &'static str> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "source_unavailable")?;
    conn.busy_timeout(Duration::from_millis(150))
        .map_err(sqlite_error)?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN;")
        .map_err(sqlite_error)?;
    let table: String = conn
        .query_row(
            "SELECT type FROM sqlite_schema WHERE name='credential'",
            [],
            |r| r.get(0),
        )
        .map_err(sqlite_error)?;
    if table != "table" {
        return Err("unsupported_schema");
    }
    // OAuth secrets are never selected, and values from other integrations never
    // leave SQLite. Bound both account count and per-key JSON size.
    let mut stmt = conn.prepare(
        "SELECT id,label,active,
          CASE WHEN json_valid(value) THEN json_extract(value,'$.type') ELSE 'invalid' END,
          CASE WHEN json_valid(value) AND length(value)<=65536 AND json_extract(value,'$.type')='key' THEN value ELSE NULL END,
          time_updated
         FROM credential WHERE integration_id='opencode' ORDER BY id LIMIT 101"
    ).map_err(sqlite_error)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Source {
                id: r.get(0)?,
                label: r.get(1)?,
                active: r.get(2)?,
                kind: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                key_value: r.get(4)?,
                updated: r.get(5)?,
            })
        })
        .map_err(sqlite_error)?;
    let rows = rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?;
    if rows.len() > MAX_ROWS {
        return Err("too_many_credentials");
    }
    Ok(rows)
}
fn entry_id(source_id: &str) -> String {
    hex::encode(Sha256::digest(source_id.as_bytes()))
}
fn fingerprint(rows: &[Source]) -> Vec<u8> {
    let mut digest = Sha256::new();
    for row in rows {
        for field in [
            &row.id,
            &row.label,
            &row.kind,
            &row.active.map(|v| v.to_string()).unwrap_or_default(),
            &row.updated.to_string(),
            &row.key_value.clone().unwrap_or_default(),
        ] {
            digest.update((field.len() as u64).to_le_bytes());
            digest.update(field.as_bytes());
        }
    }
    digest.finalize().to_vec()
}
#[cfg(test)]
#[path = "opencode_local_import_tests.rs"]
mod tests;
