//! SQLite owns imported IDs and next-session selection. A session snapshot keeps
//! every request in one process/context on the same selection until restart.
use crate::database::Database;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const SETTING: &str = "zcode_device_ids_v1";
const MAX_IDS: usize = 100;
const MAX_INPUT: usize = 64 * 1024;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    devices: Vec<String>,
    selected_device_mid: Option<String>,
    revision: u64,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    scopes: BTreeMap<String, Catalog>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    devices: Vec<String>,
    selected_device_mid: Option<String>,
    effective_device_mid: Option<String>,
    restart_required: bool,
    revision: u64,
}
fn normalize_id(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    let id = uuid::Uuid::parse_str(value).map_err(|_| "invalid")?;
    let canonical = id.hyphenated().to_string();
    if id.is_nil()
        || id.get_variant() != uuid::Variant::RFC4122
        || id.get_version().is_none()
        || !canonical.eq_ignore_ascii_case(value)
    {
        return Err("invalid");
    }
    Ok(canonical)
}
fn parse_ids(input: &str) -> Result<Vec<String>, &'static str> {
    if input.len() > MAX_INPUT || input.trim().is_empty() {
        return Err("invalid");
    }
    let input = input.trim();
    let raw: Vec<String> = if matches!(input.as_bytes().first(), Some(b'[' | b'{' | b'"')) {
        let value: serde_json::Value = serde_json::from_str(input).map_err(|_| "invalid")?;
        let item = |v: &serde_json::Value| {
            v.as_str()
                .or_else(|| {
                    v.get("device_mid")
                        .or_else(|| v.get("deviceMid"))
                        .and_then(serde_json::Value::as_str)
                })
                .map(String::from)
                .ok_or("invalid")
        };
        if let Some(profiles) = value.get("profiles") {
            profiles
                .as_array()
                .ok_or("invalid")?
                .iter()
                .map(item)
                .collect::<Result<_, _>>()?
        } else if let Some(items) = value.as_array() {
            items.iter().map(item).collect::<Result<_, _>>()?
        } else {
            vec![item(&value)?]
        }
    } else {
        input
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    };
    if raw.is_empty() || raw.len() > MAX_IDS {
        return Err("invalid");
    }
    let mut ids = Vec::new();
    for value in raw {
        let id = normalize_id(&value)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}
fn load(db: &Database) -> Result<Saved, &'static str> {
    let saved: Saved = match db.get_setting(SETTING).map_err(|_| "unavailable")? {
        Some(raw) => serde_json::from_str(&raw).map_err(|_| "unavailable")?,
        None => Saved::default(),
    };
    for catalog in saved.scopes.values() {
        if catalog.devices.len() > MAX_IDS
            || catalog
                .devices
                .iter()
                .any(|id| normalize_id(id).as_ref() != Ok(id))
            || catalog
                .devices
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != catalog.devices.len()
            || catalog
                .selected_device_mid
                .as_ref()
                .is_some_and(|id| !catalog.devices.contains(id))
        {
            return Err("unavailable");
        }
    }
    Ok(saved)
}
fn persist(db: &Database, saved: &Saved) -> Result<(), &'static str> {
    let raw = serde_json::to_string(saved).map_err(|_| "unavailable")?;
    db.set_setting(SETTING, &raw).map_err(|_| "unavailable")
}
#[derive(Clone)]
struct SessionDevice {
    selected: Option<String>,
    effective: Option<String>,
}
#[derive(Default)]
struct SessionSelections(Mutex<BTreeMap<(PathBuf, String), SessionDevice>>);
impl SessionSelections {
    fn current(
        &self,
        db: &Database,
        scope: &str,
        root: Option<&Path>,
    ) -> Result<SessionDevice, &'static str> {
        let mut sessions = self.0.lock().map_err(|_| "unavailable")?;
        let key = (db.secret_session().root().to_path_buf(), scope.to_owned());
        if let Some(device) = sessions.get(&key) {
            return Ok(device.clone());
        }
        let selected = load(db)?
            .scopes
            .get(scope)
            .and_then(|c| c.selected_device_mid.clone());
        let effective = selected
            .clone()
            .or_else(|| root.and_then(super::source_context::existing_device_mid));
        let device = SessionDevice {
            selected,
            effective,
        };
        sessions.insert(key, device.clone());
        Ok(device)
    }
}
fn sessions() -> &'static SessionSelections {
    static SESSIONS: OnceLock<SessionSelections> = OnceLock::new();
    SESSIONS.get_or_init(SessionSelections::default)
}
pub(super) fn current(
    db: &Database,
    context: &super::library_context::LibraryContext,
) -> Result<Option<String>, &'static str> {
    Ok(sessions()
        .current(db, context.context_id(), Some(context.data_root()))?
        .effective)
}
fn view(
    db: &Database,
    scope: &str,
    session: &SessionSelections,
    root: Option<&Path>,
) -> Result<View, &'static str> {
    let device = session.current(db, scope, root)?;
    let catalog = load(db)?.scopes.remove(scope).unwrap_or_default();
    Ok(View {
        restart_required: device.selected != catalog.selected_device_mid,
        devices: catalog.devices,
        selected_device_mid: catalog.selected_device_mid,
        effective_device_mid: device.effective,
        revision: catalog.revision,
    })
}
fn update(
    db: &Database,
    scope: &str,
    revision: u64,
    input: Option<&str>,
    selection: Option<Option<&str>>,
    session: &SessionSelections,
    root: Option<&Path>,
) -> Result<View, &'static str> {
    // Freeze the running identity before changing the next-session setting.
    session.current(db, scope, root)?;
    let mut saved = load(db)?;
    let catalog = saved.scopes.entry(scope.into()).or_default();
    if catalog.revision != revision {
        return Err("changed");
    }
    if let Some(input) = input {
        for id in parse_ids(input)? {
            if !catalog.devices.contains(&id) {
                catalog.devices.push(id);
            }
        }
        if catalog.devices.len() > MAX_IDS {
            return Err("invalid");
        }
    }
    if let Some(selection) = selection {
        let id = selection.map(normalize_id).transpose()?;
        if id.as_ref().is_some_and(|id| !catalog.devices.contains(id)) {
            return Err("invalid");
        }
        catalog.selected_device_mid = id;
    }
    catalog.revision = catalog.revision.checked_add(1).ok_or("unavailable")?;
    persist(db, &saved)?;
    view(db, scope, session, root)
}
#[cfg(feature = "gui")]
pub(crate) async fn state(
    db: std::sync::Arc<Database>,
    root: Option<std::path::PathBuf>,
) -> Result<View, &'static str> {
    super::runtime::run_owned(db, move |db| {
        Ok((|| {
            let context = super::native_context::library_context(root.as_deref())
                .map_err(|_| "unavailable")?;
            view(
                db,
                context.context_id(),
                sessions(),
                Some(context.data_root()),
            )
        })())
    })
    .await
    .map_err(|_| "unavailable")?
}
#[cfg(feature = "gui")]
pub(crate) async fn import(
    db: std::sync::Arc<Database>,
    root: Option<std::path::PathBuf>,
    input: String,
    revision: u64,
) -> Result<View, &'static str> {
    mutate(db, root, Some(input), None, revision).await
}
#[cfg(feature = "gui")]
pub(crate) async fn select(
    db: std::sync::Arc<Database>,
    root: Option<std::path::PathBuf>,
    selection: Option<String>,
    revision: u64,
) -> Result<View, &'static str> {
    mutate(db, root, None, Some(selection), revision).await
}
#[cfg(feature = "gui")]
async fn mutate(
    db: std::sync::Arc<Database>,
    root: Option<std::path::PathBuf>,
    input: Option<String>,
    selection: Option<Option<String>>,
    revision: u64,
) -> Result<View, &'static str> {
    super::runtime::run_owned(db, move |db| {
        Ok((|| {
            let context = super::native_context::library_context(root.as_deref())
                .map_err(|_| "unavailable")?;
            update(
                db,
                context.context_id(),
                revision,
                input.as_deref(),
                selection.as_ref().map(|s| s.as_deref()),
                sessions(),
                Some(context.data_root()),
            )
        })())
    })
    .await
    .map_err(|_| "unavailable")?
}
#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "11111111-2222-4333-8444-555555555555";
    const B: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    #[test]
    fn device_selection_protection_is_registered() {
        assert!(crate::secrets::inventory::is_protected_setting(SETTING));
    }
    #[test]
    fn local_identity_including_absence_is_frozen_until_restart() {
        let db = Database::memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("telemetry-state.json");
        let running = SessionSelections::default();
        assert!(running
            .current(&db, "a", Some(root.path()))
            .unwrap()
            .effective
            .is_none());
        std::fs::write(&path, format!(r#"{{"deviceMid":"{A}"}}"#)).unwrap();
        assert!(running
            .current(&db, "a", Some(root.path()))
            .unwrap()
            .effective
            .is_none());
        let restarted = SessionSelections::default();
        assert_eq!(
            restarted
                .current(&db, "a", Some(root.path()))
                .unwrap()
                .effective
                .as_deref(),
            Some(A)
        );
        std::fs::write(&path, format!(r#"{{"deviceMid":"{B}"}}"#)).unwrap();
        assert_eq!(
            restarted
                .current(&db, "a", Some(root.path()))
                .unwrap()
                .effective
                .as_deref(),
            Some(A)
        );
        assert_eq!(
            SessionSelections::default()
                .current(&db, "a", Some(root.path()))
                .unwrap()
                .effective
                .as_deref(),
            Some(B)
        );
    }
    #[test]
    fn cache_does_not_cross_database_storage_namespaces() {
        let first = Database::memory().unwrap();
        let second = Database::memory().unwrap();
        let cache = SessionSelections::default();
        update(
            &first,
            "a",
            0,
            Some(A),
            None,
            &SessionSelections::default(),
            None,
        )
        .unwrap();
        update(
            &first,
            "a",
            1,
            None,
            Some(Some(A)),
            &SessionSelections::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            cache
                .current(&first, "a", None)
                .unwrap()
                .effective
                .as_deref(),
            Some(A)
        );
        assert!(cache
            .current(&second, "a", None)
            .unwrap()
            .effective
            .is_none());
    }
    #[test]
    fn imported_ids_are_encrypted_at_rest_and_removed_by_protected_reset() {
        let db = Database::memory().unwrap();
        update(
            &db,
            "a",
            0,
            Some(A),
            None,
            &SessionSelections::default(),
            None,
        )
        .unwrap();
        let conn = db.conn.lock().unwrap();
        let raw: String = conn
            .query_row("SELECT value FROM settings WHERE key=?1", [SETTING], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(!raw.contains(A));
        assert!(serde_json::from_str::<serde_json::Value>(&raw).is_err());
        crate::secrets::inventory::reset_database(
            &conn,
            &crate::secrets::VaultContext::generate().unwrap(),
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key=?1",
                [SETTING],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }
    #[test]
    fn twenty_id_fixture_batch_is_deduplicated_and_bounded() {
        let ids: Vec<_> = (0..20).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let profiles: Vec<_> = ids
            .iter()
            .map(|id| serde_json::json!({"device_mid":id}))
            .collect();
        let text = serde_json::json!({"profiles":profiles,"observed_host_context":{"os_version":"synthetic"}}).to_string();
        assert_eq!(parse_ids(&text).unwrap(), ids);
        assert!(parse_ids(&serde_json::to_string(&vec![A; MAX_IDS + 1]).unwrap()).is_err());
        assert!(normalize_id("00000000-0000-0000-0000-000000000001").is_err());
    }
    #[test]
    fn imports_fixture_json_without_treating_other_metadata_as_identifiers() {
        let input = format!(
            r#"{{"profiles":[{{"device_mid":"{A}"}}],"observed_host_context":{{"app_version":"99.1.2"}}}}"#
        );
        assert_eq!(parse_ids(&input).unwrap(), vec![A]);
        assert_eq!(
            parse_ids(&format!("{A}\n{}, {B}", A.to_uppercase())).unwrap(),
            vec![A, B]
        );
        assert_eq!(
            parse_ids(&format!(r#"{{"deviceMid":"{A}"}}"#)).unwrap(),
            vec![A]
        );
    }
    #[test]
    fn invalid_imports_are_atomic_and_do_not_replace_persisted_ids() {
        let db = Database::memory().unwrap();
        let cache = SessionSelections::default();
        update(&db, "a", 0, Some(A), None, &cache, None).unwrap();
        let before = db.get_setting(SETTING).unwrap();
        for input in [
            format!("{B}\ninvalid"),
            "null".into(),
            r#"{"profiles":[{"token":"secret-canary"}]}"#.into(),
            "0".repeat(MAX_INPUT + 1),
        ] {
            assert!(update(&db, "a", 1, Some(&input), None, &cache, None).is_err());
            assert_eq!(db.get_setting(SETTING).unwrap(), before);
        }
        assert_eq!(
            update(&db, "a", 0, Some(B), None, &cache, None).err(),
            Some("changed")
        );
        assert!(normalize_id("00000000-0000-0000-0000-000000000000").is_err());
    }
    #[test]
    fn sqlite_selection_is_scoped_and_applies_only_in_a_new_session() {
        let db = Database::memory().unwrap();
        let running = SessionSelections::default();
        update(
            &db,
            "a",
            0,
            Some(&format!("{A}\n{B}")),
            None,
            &running,
            None,
        )
        .unwrap();
        let changed = update(&db, "a", 1, None, Some(Some(A)), &running, None).unwrap();
        assert_eq!(changed.selected_device_mid.as_deref(), Some(A));
        assert!(changed.restart_required);
        assert_eq!(running.current(&db, "a", None).unwrap().effective, None);
        let restarted = SessionSelections::default();
        assert_eq!(
            restarted
                .current(&db, "a", None)
                .unwrap()
                .effective
                .as_deref(),
            Some(A)
        );
        assert!(!view(&db, "a", &restarted, None).unwrap().restart_required);
        assert_eq!(restarted.current(&db, "b", None).unwrap().effective, None);
        assert!(view(&db, "b", &restarted, None).unwrap().devices.is_empty());
        let native = update(&db, "a", 2, None, Some(None), &restarted, None).unwrap();
        assert!(native.restart_required);
        assert_eq!(
            restarted
                .current(&db, "a", None)
                .unwrap()
                .effective
                .as_deref(),
            Some(A)
        );
        assert_eq!(
            SessionSelections::default()
                .current(&db, "a", None)
                .unwrap()
                .effective,
            None
        );
    }
    #[test]
    fn unknown_selection_and_corrupt_database_fail_closed() {
        let db = Database::memory().unwrap();
        let cache = SessionSelections::default();
        assert_eq!(
            update(&db, "a", 0, None, Some(Some(A)), &cache, None).err(),
            Some("invalid")
        );
        assert!(db.get_setting(SETTING).unwrap().is_none());
        db.set_setting(SETTING, "invalid").unwrap();
        assert_eq!(load(&db).err(), Some("unavailable"));
    }
}
