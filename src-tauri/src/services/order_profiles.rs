//! Named order snapshots and their applied identity. Reads only project state;
//! explicit mutations commit profiles, current identity and routing order together.
use crate::{
    app_config::AppType, database::lock_conn, error::AppError, proxy::application_routing, Database,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::HashSet, str::FromStr};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrderProfile {
    pub name: String,
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrderProfilesState {
    pub profiles: Vec<OrderProfile>,
    pub current: String,
}

fn profiles_key(app: &str) -> String {
    format!("application_order_profiles_{app}")
}
fn current_key(app: &str) -> String {
    format!("application_order_profile_current_{app}")
}

fn read_json<T: DeserializeOwned>(conn: &Connection, key: &str) -> Result<Option<T>, AppError> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?;
    raw.filter(|raw| !raw.is_empty())
        .map(|raw| serde_json::from_str(&raw).map_err(|e| AppError::Config(e.to_string())))
        .transpose()
}

fn write_json(conn: &Connection, key: &str, value: &impl Serialize) -> Result<(), AppError> {
    let raw = serde_json::to_string(value).map_err(|e| AppError::Config(e.to_string()))?;
    conn.execute(
        "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, raw],
    )?;
    Ok(())
}

fn default_profile(conn: &Connection, app: &str) -> Result<OrderProfile, AppError> {
    Ok(OrderProfile {
        name: "default".into(),
        provider_ids: application_routing::chain_ids_on(conn, app)?,
    })
}

fn read_state(conn: &Connection, app: &str) -> Result<OrderProfilesState, AppError> {
    AppType::from_str(app)?;
    let mut profiles: Vec<OrderProfile> = read_json(conn, &profiles_key(app))?.unwrap_or_default();
    let mut seen = HashSet::new();
    profiles.retain(|profile| !profile.name.trim().is_empty() && seen.insert(profile.name.clone()));
    if profiles.is_empty() {
        profiles.push(default_profile(conn, app)?);
    }
    let current = read_json::<String>(conn, &current_key(app))?
        .filter(|name| profiles.iter().any(|profile| profile.name == *name))
        .unwrap_or_else(|| profiles[0].name.clone());
    Ok(OrderProfilesState { profiles, current })
}

pub fn get(db: &Database, app: &str) -> Result<OrderProfilesState, AppError> {
    let conn = lock_conn!(db.conn);
    read_state(&conn, app)
}

fn mutate(
    db: &Database,
    app: &str,
    change: impl FnOnce(&Transaction<'_>, &mut OrderProfilesState) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    let mut state = read_state(&tx, app)?;
    change(&tx, &mut state)?;
    write_json(&tx, &profiles_key(app), &state.profiles)?;
    write_json(&tx, &current_key(app), &state.current)?;
    tx.commit()?;
    Ok(())
}

fn profile_name(name: &str) -> Result<&str, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Config("Profile name is empty".into()));
    }
    Ok(name)
}

fn validate_target(
    conn: &Connection,
    state: &OrderProfilesState,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<usize, AppError> {
    let name = profile_name(name)?;
    application_routing::validate_order_on(conn, app, ids)?;
    state
        .profiles
        .iter()
        .position(|profile| profile.name == name)
        .ok_or_else(|| AppError::Config(format!("Profile not found: {name}")))
}

/// Pure preflight before user confirmation or native application changes.
pub fn validate_order(
    db: &Database,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    let state = read_state(&conn, app)?;
    validate_target(&conn, &state, app, name, ids).map(|_| ())
}

fn apply_on(
    conn: &Connection,
    state: &mut OrderProfilesState,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<(), AppError> {
    let index = validate_target(conn, state, app, name, ids)?;
    state.profiles[index].provider_ids = ids.to_vec();
    state.current = state.profiles[index].name.clone();
    application_routing::write_order_on(conn, app, ids)
}

/// Revalidates the named target at commit time; a removed draft is never recreated.
pub fn apply_order(db: &Database, app: &str, name: &str, ids: &[String]) -> Result<(), AppError> {
    mutate(db, app, |tx, state| apply_on(tx, state, app, name, ids))
}

pub fn apply_current_order(db: &Database, app: &str, ids: &[String]) -> Result<(), AppError> {
    mutate(db, app, |tx, state| {
        apply_on(tx, state, app, &state.current.clone(), ids)
    })
}

/// Saving a snapshot does not apply its order or change the applied identity.
pub fn save(db: &Database, app: &str, name: &str, ids: &[String]) -> Result<(), AppError> {
    let name = profile_name(name)?.to_string();
    mutate(db, app, |tx, state| {
        application_routing::validate_order_on(tx, app, ids)?;
        upsert(
            &mut state.profiles,
            OrderProfile {
                name,
                provider_ids: ids.to_vec(),
            },
        );
        Ok(())
    })
}

fn upsert(profiles: &mut Vec<OrderProfile>, profile: OrderProfile) {
    if let Some(existing) = profiles
        .iter_mut()
        .find(|existing| existing.name == profile.name)
    {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
}

pub fn rename(db: &Database, app: &str, from: &str, to: &str) -> Result<(), AppError> {
    let from = profile_name(from)?;
    let to = profile_name(to)?;
    mutate(db, app, |_, state| {
        if from != to && state.profiles.iter().any(|profile| profile.name == to) {
            return Err(AppError::Config(format!("Profile already exists: {to}")));
        }
        let profile = state
            .profiles
            .iter_mut()
            .find(|profile| profile.name == from)
            .ok_or_else(|| AppError::Config(format!("Profile not found: {from}")))?;
        profile.name = to.into();
        if state.current == from {
            state.current = to.into();
        }
        Ok(())
    })
}

pub fn remove(db: &Database, app: &str, name: &str) -> Result<(), AppError> {
    let name = profile_name(name)?;
    mutate(db, app, |tx, state| {
        let index = state
            .profiles
            .iter()
            .position(|profile| profile.name == name)
            .ok_or_else(|| AppError::Config(format!("Profile not found: {name}")))?;
        state.profiles.remove(index);
        if state.profiles.is_empty() {
            state.profiles.push(default_profile(tx, app)?);
        }
        if !state
            .profiles
            .iter()
            .any(|profile| profile.name == state.current)
        {
            state.current = state.profiles[0].name.clone();
        }
        Ok(())
    })
}

/// Import is all-or-nothing. Invalid members are reported, never silently dropped.
pub fn import(db: &Database, app: &str, profiles: Vec<OrderProfile>) -> Result<usize, AppError> {
    let count = profiles.len();
    if count == 0 {
        return Err(AppError::Config("Profile file is empty".into()));
    }
    mutate(db, app, |tx, state| {
        let mut names = HashSet::new();
        for mut profile in profiles {
            profile.name = profile_name(&profile.name)?.to_string();
            if !names.insert(profile.name.clone()) {
                return Err(AppError::Config(
                    "Profile file contains duplicate names".into(),
                ));
            }
            application_routing::validate_order_on(tx, app, &profile.provider_ids)?;
            upsert(&mut state.profiles, profile);
        }
        Ok(())
    })?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Database {
        let db = Database::memory().unwrap();
        for id in ["a", "b"] {
            db.save_provider(
                "claude",
                &crate::provider::Provider::with_id(
                    id.into(),
                    id.into(),
                    serde_json::json!({}),
                    None,
                ),
            )
            .unwrap();
        }
        db
    }

    fn reject_pointer(db: &Database) {
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_pointer BEFORE INSERT ON settings WHEN NEW.key = 'application_order_profile_current_claude' BEGIN SELECT RAISE(ABORT, 'pointer rejected'); END;").unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn profile_reads_do_not_persist_defaults() {
        let db = database();
        validate_order(&db, "claude", "default", &["a".into()]).unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.current, "default");
        assert_eq!(state.profiles[0].provider_ids, vec!["a", "b"]);
        assert!(db.get_setting(&profiles_key("claude")).unwrap().is_none());
        assert!(db.get_setting(&current_key("claude")).unwrap().is_none());
        assert!(db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_none());
    }

    #[test]
    #[serial_test::serial]
    fn invalid_pointer_resolves_existing_profile_without_writing() {
        let db = database();
        rename(&db, "claude", "default", "daily").unwrap();
        db.set_setting(&current_key("claude"), "\"removed\"")
            .unwrap();
        assert_eq!(get(&db, "claude").unwrap().current, "daily");
        assert_eq!(
            db.get_setting(&current_key("claude")).unwrap().as_deref(),
            Some("\"removed\"")
        );
    }

    #[test]
    #[serial_test::serial]
    fn save_and_read_leave_applied_order_until_commit() {
        let db = database();
        save(&db, "claude", "backup", &["b".into()]).unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.current, "default");
        assert_eq!(
            application_routing::chain_ids(&db, "claude").unwrap(),
            vec!["a", "b"]
        );
        apply_order(&db, "claude", "backup", &["b".into(), "a".into()]).unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.current, "backup");
        assert_eq!(state.profiles[1].provider_ids, vec!["b", "a"]);
        assert_eq!(
            application_routing::chain_ids(&db, "claude").unwrap(),
            vec!["b", "a"]
        );
    }

    #[test]
    #[serial_test::serial]
    fn legacy_order_command_updates_the_applied_profile() {
        let db = database();
        save(&db, "claude", "daily", &["b".into()]).unwrap();
        apply_order(&db, "claude", "daily", &["b".into()]).unwrap();
        application_routing::set_order(&db, "claude", &["a".into()]).unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.current, "daily");
        assert_eq!(state.profiles[1].provider_ids, vec!["a"]);
    }

    #[test]
    #[serial_test::serial]
    fn apply_failure_rolls_back_chain_profile_and_pointer() {
        let db = database();
        apply_order(&db, "claude", "default", &["a".into()]).unwrap();
        save(&db, "claude", "backup", &["b".into()]).unwrap();
        let before = get(&db, "claude").unwrap();
        reject_pointer(&db);
        assert!(apply_order(&db, "claude", "backup", &["b".into(), "a".into()]).is_err());
        assert_eq!(get(&db, "claude").unwrap(), before);
        assert_eq!(
            application_routing::chain_ids(&db, "claude").unwrap(),
            vec!["a"]
        );
    }

    #[test]
    #[serial_test::serial]
    fn rename_failure_does_not_leave_a_dangling_pointer() {
        let db = database();
        apply_order(&db, "claude", "default", &["a".into()]).unwrap();
        reject_pointer(&db);
        assert!(rename(&db, "claude", "default", "daily").is_err());
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.profiles[0].name, "default");
        assert_eq!(state.current, "default");
    }

    #[test]
    #[serial_test::serial]
    fn rename_and_delete_keep_current_valid() {
        let db = database();
        save(&db, "claude", "backup", &["b".into()]).unwrap();
        assert!(rename(&db, "claude", "backup", "default").is_err());
        rename(&db, "claude", "default", "daily").unwrap();
        assert_eq!(get(&db, "claude").unwrap().current, "daily");
        remove(&db, "claude", "daily").unwrap();
        assert_eq!(get(&db, "claude").unwrap().current, "backup");
        remove(&db, "claude", "backup").unwrap();
        assert_eq!(get(&db, "claude").unwrap().current, "default");
        assert!(apply_order(&db, "claude", "backup", &["b".into()]).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn invalid_orders_never_change_state() {
        let db = database();
        let before = get(&db, "claude").unwrap();
        for ids in [vec![], vec!["unknown".into()], vec!["a".into(), "a".into()]] {
            assert!(validate_order(&db, "claude", "default", &ids).is_err());
            assert!(apply_order(&db, "claude", "default", &ids).is_err());
            assert!(save(&db, "claude", "invalid", &ids).is_err());
            assert_eq!(get(&db, "claude").unwrap(), before);
        }
        assert!(save(&db, "claude", " ", &["a".into()]).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn import_validates_entire_file_before_commit() {
        let db = database();
        let before = get(&db, "claude").unwrap();
        let profile = |name: &str, id: &str| OrderProfile {
            name: name.into(),
            provider_ids: vec![id.into()],
        };
        assert!(import(
            &db,
            "claude",
            vec![profile("valid", "a"), profile("invalid", "unknown")]
        )
        .is_err());
        assert_eq!(get(&db, "claude").unwrap(), before);
        assert!(import(
            &db,
            "claude",
            vec![profile("same", "a"), profile(" same ", "b")]
        )
        .is_err());
        assert_eq!(get(&db, "claude").unwrap(), before);
        assert_eq!(
            import(&db, "claude", vec![profile("daily", "b")]).unwrap(),
            1
        );
        assert_eq!(get(&db, "claude").unwrap().current, "default");
    }
}
