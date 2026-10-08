//! Named order snapshots and their applied identity. Reads only project state;
//! explicit mutations commit profiles, current identity and routing order together.
//! Provider inventory changes maintain references on the same SQLite connection.
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
    write_state(&tx, app, &mut state)?;
    tx.commit()?;
    Ok(())
}

/// Empty defaults are a presentation placeholder, never a saved snapshot.
fn write_state(
    conn: &Connection,
    app: &str,
    state: &mut OrderProfilesState,
) -> Result<(), AppError> {
    normalize_state(state);
    write_json(conn, &profiles_key(app), &state.profiles)?;
    write_json(conn, &current_key(app), &state.current)
}

fn normalize_state(state: &mut OrderProfilesState) {
    state
        .profiles
        .retain(|profile| !profile.provider_ids.is_empty());
    if !state
        .profiles
        .iter()
        .any(|profile| profile.name == state.current)
    {
        state.current = state
            .profiles
            .first()
            .map(|profile| profile.name.clone())
            .unwrap_or_else(|| "default".into());
    }
}

/// Maintain references after the final inventory is known. The caller owns the
/// transaction; temporary removal followed by restoration must finish first.
pub(crate) fn reconcile_on(conn: &Connection, app: &str) -> Result<(), AppError> {
    let mut statement = conn.prepare("SELECT id FROM providers WHERE app_type = ?1")?;
    let known = statement
        .query_map([app], |row| row.get::<_, String>(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    let mut state = read_state(conn, app)?;
    let mut chain = application_routing::chain_ids_on(conn, app)?;
    chain.retain(|id| known.contains(id));
    // Persist [] rather than deleting the key: no selected survivors is not an
    // uninitialized order and must not fall back to the whole inventory.
    application_routing::write_order_on(conn, app, &chain)?;
    for profile in &mut state.profiles {
        profile.provider_ids.retain(|id| known.contains(id));
    }
    state
        .profiles
        .retain(|profile| !profile.provider_ids.is_empty());
    if state.profiles.is_empty() && !chain.is_empty() {
        state.profiles.push(OrderProfile {
            name: "default".into(),
            provider_ids: chain,
        });
    }
    write_state(conn, app, &mut state)
}

/// Return the complete validated snapshot before the desktop opens its dialog.
pub fn export_json(db: &Database, app: &str) -> Result<String, AppError> {
    let conn = lock_conn!(db.conn);
    let state = read_state(&conn, app)?;
    for profile in &state.profiles {
        application_routing::validate_order_on(&conn, app, &profile.provider_ids)?;
    }
    serde_json::to_string_pretty(&state.profiles)
        .map_err(|error| AppError::Config(error.to_string()))
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

fn select_order_on(
    conn: &Connection,
    state: &mut OrderProfilesState,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<(), AppError> {
    let index = validate_target(conn, state, app, name, ids)?;
    state.profiles[index].provider_ids = ids.to_vec();
    state.current = state.profiles[index].name.clone();
    Ok(())
}

fn apply_on(
    conn: &Connection,
    state: &mut OrderProfilesState,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<(), AppError> {
    select_order_on(conn, state, app, name, ids)?;
    application_routing::write_order_on(conn, app, ids)
}

/// Exact evidence for the original three-key owner, including absent settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderSnapshot {
    pub profiles: Option<String>,
    pub current: Option<String>,
    pub priority: Option<String>,
}

fn snapshot_on(conn: &Connection, app: &str) -> Result<OrderSnapshot, AppError> {
    let read = |key: String| -> Result<Option<String>, AppError> {
        Ok(conn
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    };
    Ok(OrderSnapshot {
        profiles: read(profiles_key(app))?,
        current: read(current_key(app))?,
        priority: read(application_routing::priority_key(app))?,
    })
}

fn planned_on(
    conn: &Connection,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<OrderSnapshot, AppError> {
    let mut state = read_state(conn, app)?;
    select_order_on(conn, &mut state, app, name, ids)?;
    normalize_state(&mut state);
    Ok(OrderSnapshot {
        profiles: Some(
            serde_json::to_string(&state.profiles).map_err(|e| AppError::Config(e.to_string()))?,
        ),
        current: Some(
            serde_json::to_string(&state.current).map_err(|e| AppError::Config(e.to_string()))?,
        ),
        priority: Some(serde_json::to_string(ids).map_err(|e| AppError::Config(e.to_string()))?),
    })
}

pub(crate) fn prepare_apply(
    db: &Database,
    app: &str,
    name: &str,
    ids: &[String],
) -> Result<(OrderSnapshot, OrderSnapshot), AppError> {
    let conn = lock_conn!(db.conn);
    Ok((snapshot_on(&conn, app)?, planned_on(&conn, app, name, ids)?))
}

fn verify_prepared_on(
    conn: &Connection,
    app: &str,
    name: &str,
    ids: &[String],
    before: &OrderSnapshot,
    planned: &OrderSnapshot,
) -> Result<bool, AppError> {
    let actual = snapshot_on(conn, app)?;
    application_routing::validate_order_on(conn, app, ids)?;
    if actual == *planned {
        return Ok(true);
    }
    if actual != *before || planned_on(conn, app, name, ids)? != *planned {
        return Err(AppError::Config("mode.routing_order_changed".into()));
    }
    Ok(false)
}

pub(crate) fn verify_prepared(
    db: &Database,
    app: &str,
    name: &str,
    ids: &[String],
    before: &OrderSnapshot,
    planned: &OrderSnapshot,
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    verify_prepared_on(&conn, app, name, ids, before, planned).map(|_| ())
}

/// Compare, mutate and read back under the original DAO's single SQLite transaction.
pub(crate) fn apply_prepared(
    db: &Database,
    app: &str,
    name: &str,
    ids: &[String],
    before: &OrderSnapshot,
    planned: &OrderSnapshot,
) -> Result<(), AppError> {
    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    if verify_prepared_on(&tx, app, name, ids, before, planned)? {
        return Ok(());
    }
    let mut state = read_state(&tx, app)?;
    apply_on(&tx, &mut state, app, name, ids)?;
    write_state(&tx, app, &mut state)?;
    if snapshot_on(&tx, app)? != *planned {
        return Err(AppError::Config("mode.routing_order_changed".into()));
    }
    tx.commit()?;
    Ok(())
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

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_original_tests() {
    tests::profile_reads_do_not_persist_defaults();
    tests::invalid_pointer_resolves_existing_profile_without_writing();
    tests::save_and_read_leave_applied_order_until_commit();
    tests::legacy_order_command_updates_the_applied_profile();
    tests::apply_failure_rolls_back_chain_profile_and_pointer();
    tests::rename_failure_does_not_leave_a_dangling_pointer();
    tests::rename_and_delete_keep_current_valid();
    tests::invalid_orders_never_change_state();
    tests::import_validates_entire_file_before_commit();
    tests::exported_json_round_trip_preserves_named_members_without_applying_them();
    tests::invalid_imported_file_never_overwrites_an_existing_snapshot_or_applied_chain();
    tests::deleting_provider_prunes_snapshots_and_preserves_surviving_identity();
    tests::deleting_last_chain_member_never_expands_to_unselected_providers();
    tests::deletion_falls_back_only_when_the_current_snapshot_loses_every_member();
    tests::failed_reference_maintenance_rolls_back_provider_deletion();
    tests::startup_repairs_historical_references_while_get_stays_read_only();
    println!("PASS original named-order owner tests");
}

#[cfg(any(test, feature = "test-hooks"))]
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn profile_reads_do_not_persist_defaults() {
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn invalid_pointer_resolves_existing_profile_without_writing() {
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn save_and_read_leave_applied_order_until_commit() {
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn legacy_order_command_updates_the_applied_profile() {
        let db = database();
        save(&db, "claude", "daily", &["b".into()]).unwrap();
        apply_order(&db, "claude", "daily", &["b".into()]).unwrap();
        application_routing::set_order(&db, "claude", &["a".into()]).unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.current, "daily");
        assert_eq!(state.profiles[1].provider_ids, vec!["a"]);
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn apply_failure_rolls_back_chain_profile_and_pointer() {
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn rename_failure_does_not_leave_a_dangling_pointer() {
        let db = database();
        apply_order(&db, "claude", "default", &["a".into()]).unwrap();
        reject_pointer(&db);
        assert!(rename(&db, "claude", "default", "daily").is_err());
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.profiles[0].name, "default");
        assert_eq!(state.current, "default");
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn rename_and_delete_keep_current_valid() {
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn invalid_orders_never_change_state() {
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn import_validates_entire_file_before_commit() {
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

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn exported_json_round_trip_preserves_named_members_without_applying_them() {
        let source = database();
        save(
            &source,
            "claude",
            "Travel \"备用\"",
            &["b".into(), "a".into()],
        )
        .unwrap();
        save(&source, "claude", "Single", &["b".into()]).unwrap();
        let exported = get(&source, "claude").unwrap().profiles;
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            serde_json::to_string_pretty(&exported).unwrap(),
        )
        .unwrap();

        let destination = database();
        save(&destination, "claude", "Travel \"备用\"", &["a".into()]).unwrap();
        apply_order(&destination, "claude", "default", &["a".into()]).unwrap();
        let raw = std::fs::read_to_string(file.path()).unwrap();
        let parsed: Vec<OrderProfile> = serde_json::from_str(&raw).unwrap();
        assert_eq!(import(&destination, "claude", parsed).unwrap(), 3);
        let imported = get(&destination, "claude").unwrap();
        assert_eq!(imported.profiles, exported);
        assert_eq!(imported.current, "default");
        assert_eq!(
            application_routing::chain_ids(&destination, "claude").unwrap(),
            vec!["a"]
        );
        apply_order(
            &destination,
            "claude",
            "Travel \"备用\"",
            &["b".into(), "a".into()],
        )
        .unwrap();
        assert_eq!(
            application_routing::chain_ids(&destination, "claude").unwrap(),
            vec!["b", "a"]
        );
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn invalid_imported_file_never_overwrites_an_existing_snapshot_or_applied_chain() {
        let db = database();
        save(&db, "claude", "Daily", &["a".into()]).unwrap();
        apply_order(&db, "claude", "Daily", &["a".into()]).unwrap();
        let before = get(&db, "claude").unwrap();
        for invalid in [
            r#"[]"#,
            r#"[{"name":"Daily","providerIds":["b"]},{"name":"Empty","providerIds":[]}]"#,
            r#"[{"name":"Daily","providerIds":["b"]},{"name":"Duplicate","providerIds":["a","a"]}]"#,
            r#"[{"name":"Daily","providerIds":["b"]},{"name":"Missing","providerIds":["removed"]}]"#,
        ] {
            let parsed: Vec<OrderProfile> = serde_json::from_str(invalid).unwrap();
            assert!(import(&db, "claude", parsed).is_err());
            assert_eq!(get(&db, "claude").unwrap(), before);
            assert_eq!(
                application_routing::chain_ids(&db, "claude").unwrap(),
                vec!["a"]
            );
        }
    }
    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn deleting_provider_prunes_snapshots_and_preserves_surviving_identity() {
        let db = database();
        save(&db, "claude", "Travel", &["b".into(), "a".into()]).unwrap();
        save(&db, "claude", "Single", &["b".into()]).unwrap();
        apply_order(&db, "claude", "Travel", &["a".into(), "b".into()]).unwrap();
        // Saving over the active name does not apply its new snapshot.
        save(&db, "claude", "Travel", &["b".into(), "a".into()]).unwrap();
        db.delete_provider("claude", "b").unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.current, "Travel");
        assert_eq!(
            state
                .profiles
                .iter()
                .map(|p| (p.name.as_str(), p.provider_ids.clone()))
                .collect::<Vec<_>>(),
            vec![("default", vec!["a".into()]), ("Travel", vec!["a".into()])]
        );
        assert_eq!(
            application_routing::chain_ids(&db, "claude").unwrap(),
            vec!["a"]
        );
        let parsed = serde_json::from_str(&export_json(&db, "claude").unwrap()).unwrap();
        import(&db, "claude", parsed).unwrap();
        assert_eq!(get(&db, "claude").unwrap(), state);
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn deleting_last_chain_member_never_expands_to_unselected_providers() {
        let db = database();
        apply_order(&db, "claude", "default", &["b".into()]).unwrap();
        db.delete_provider("claude", "b").unwrap();
        assert!(application_routing::chain_ids(&db, "claude")
            .unwrap()
            .is_empty());
        assert!(get(&db, "claude").unwrap().profiles[0]
            .provider_ids
            .is_empty());
        assert!(export_json(&db, "claude").is_err());
        application_routing::migrate(&db, "claude").unwrap();
        assert!(application_routing::chain_ids(&db, "claude")
            .unwrap()
            .is_empty());
        // Saving a new snapshot must not persist the empty display placeholder.
        save(&db, "claude", "Single", &["a".into()]).unwrap();
        assert_eq!(get(&db, "claude").unwrap().profiles.len(), 1);
        assert!(export_json(&db, "claude").is_ok());
        remove(&db, "claude", "Single").unwrap();
        // An explicit application can still populate the empty default draft.
        apply_order(&db, "claude", "default", &["a".into()]).unwrap();
        db.delete_provider("claude", "a").unwrap();
        assert!(export_json(&db, "claude").is_err());
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn deletion_falls_back_only_when_the_current_snapshot_loses_every_member() {
        let db = database();
        save(&db, "claude", "Single", &["b".into()]).unwrap();
        apply_order(&db, "claude", "Single", &["b".into()]).unwrap();
        db.delete_provider("claude", "b").unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.current, "default");
        assert_eq!(state.profiles.len(), 1);
        assert_eq!(state.profiles[0].provider_ids, vec!["a"]);
        assert!(application_routing::chain_ids(&db, "claude")
            .unwrap()
            .is_empty());
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn failed_reference_maintenance_rolls_back_provider_deletion() {
        let db = database();
        apply_order(&db, "claude", "default", &["a".into(), "b".into()]).unwrap();
        let before = get(&db, "claude").unwrap();
        reject_pointer(&db);
        assert!(db.delete_provider("claude", "b").is_err());
        assert!(db.get_provider_by_id("b", "claude").unwrap().is_some());
        assert_eq!(get(&db, "claude").unwrap(), before);
        assert_eq!(
            application_routing::chain_ids(&db, "claude").unwrap(),
            vec!["a", "b"]
        );
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn startup_repairs_historical_references_while_get_stays_read_only() {
        let db = database();
        apply_order(&db, "claude", "default", &["b".into(), "a".into()]).unwrap();
        let key = profiles_key("claude");
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM providers WHERE id='b'", [])
            .unwrap();
        let stale = db.get_setting(&key).unwrap();
        let state = get(&db, "claude").unwrap();
        assert_eq!(state.profiles[0].provider_ids, vec!["b", "a"]);
        assert_eq!(db.get_setting(&key).unwrap(), stale);
        application_routing::migrate(&db, "claude").unwrap();
        let repaired = get(&db, "claude").unwrap();
        assert_eq!(repaired.profiles[0].provider_ids, vec!["a"]);
        let persisted = db.get_setting(&key).unwrap();
        get(&db, "claude").unwrap();
        assert_eq!(db.get_setting(&key).unwrap(), persisted);
        application_routing::migrate(&db, "claude").unwrap();
        assert_eq!(get(&db, "claude").unwrap(), repaired);
        import(
            &db,
            "claude",
            serde_json::from_str(&export_json(&db, "claude").unwrap()).unwrap(),
        )
        .unwrap();
    }
}
