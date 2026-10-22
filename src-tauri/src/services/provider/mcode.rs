//! MiniMax Code custom-provider adapter. Configuration semantics adapted from
//! cc-switch v4.0.18 (MIT, Copyright (c) 2025 Jason Young).
//! This is an additive native application, not a managed-account or routing pool.
use super::SwitchResult;
use crate::error::AppError;
use crate::mcode_config::{self, ConfigBackup};
use crate::provider::{Provider, ProviderMeta};
use crate::secrets::files::OwnedFile;
use crate::store::AppState;
use indexmap::IndexMap;
use serde_json::Value;
use std::path::Path;

const APP: &str = "mcode";

/// Project native membership into saved rows without importing or writing anything.
pub(crate) fn list(state: &AppState) -> Result<IndexMap<String, Provider>, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(APP));
    let mut saved = state.db.get_all_providers(APP)?;
    let (native, revision) = mcode_config::snapshot()?;
    for (id, provider) in &mut saved {
        if let Some(config) = native.get(id) {
            provider.settings_config = config.clone();
            align_name(provider);
        }
        let meta = provider.meta.get_or_insert_with(ProviderMeta::default);
        meta.live_config_managed = Some(native.contains_key(id));
        meta.mcode_config_revision = Some(revision.clone());
    }
    Ok(saved)
}

/// The editor gets configuration and revision from the same passive native snapshot.
pub(crate) fn edit_settings(state: &AppState, id: &str) -> Result<Value, AppError> {
    let mut providers = list(state)?;
    let provider = providers
        .shift_remove(id)
        .ok_or_else(|| AppError::InvalidInput("MCode provider not found".into()))?;
    Ok(serde_json::json!({
        "settingsConfig": provider.settings_config,
        "mcodeConfigRevision": provider.meta.and_then(|meta| meta.mcode_config_revision),
        "modeState": null,
    }))
}

/// Only the explicit import command reaches this mutating path; startup/list do not.
pub(crate) fn import_from_live(state: &AppState) -> Result<usize, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(APP));
    let (native, _) = mcode_config::snapshot()?;
    // Prepare the complete selection before any DB mutation.
    let saved = state.db.get_all_providers(APP)?;
    let mut prepared = Vec::new();
    for (id, config) in &native {
        let mut provider = saved
            .get(id)
            .cloned()
            .unwrap_or_else(|| Provider::with_id(id.clone(), id.clone(), config.clone(), None));
        provider.settings_config = config.clone();
        align_name(&mut provider);
        prepare(&mut provider, true)?;
        prepared.push(provider);
    }
    let mut completed: Vec<String> = Vec::new();
    for provider in prepared {
        if let Err(error) = state.db.save_provider(APP, &provider) {
            // Import never changes the native file. Restore earlier catalog rows on failure.
            for imported in completed.iter().rev() {
                let rollback = match saved.get(imported) {
                    Some(previous) => state.db.save_provider(APP, previous),
                    None => state.db.delete_provider(APP, imported),
                };
                if rollback.is_err() {
                    return Err(AppError::Config("MCode import failed and catalog rollback is incomplete; native configuration was not changed".into()));
                }
            }
            return Err(error);
        }
        completed.push(provider.id.clone());
    }
    Ok(completed.len())
}

pub(super) fn add(state: &AppState, mut provider: Provider, live: bool) -> Result<bool, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(APP));
    write_name(&mut provider);
    prepare(&mut provider, live)?;
    if state.db.get_provider_by_id(&provider.id, APP)?.is_some()
        || mcode_config::provider_key_exists(&provider.id)?
    {
        return Err(AppError::Conflict(
            "MCode provider key already exists; use explicit import or edit".into(),
        ));
    }
    if live {
        mcode_config::transact_at(
            &mcode_config::config_path(),
            &provider.id,
            Some(provider.settings_config.clone()),
            true,
            None,
            |receipt| create_backup(state, receipt),
            || save_user_provider(state, &provider),
        )?;
    } else {
        save_user_provider(state, &provider)?;
    }
    Ok(true)
}

pub(super) fn update(
    state: &AppState,
    original_id: Option<&str>,
    mut provider: Provider,
) -> Result<bool, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(APP));
    if original_id.is_some_and(|id| id != provider.id) {
        return Err(AppError::InvalidInput(
            "MCode provider keys cannot be renamed".into(),
        ));
    }
    let previous = saved_provider(state, &provider.id)?;
    let expected = provider
        .meta
        .as_ref()
        .and_then(|meta| meta.mcode_config_revision.clone())
        .ok_or_else(|| AppError::Conflict("Refresh the MCode provider before editing".into()))?;
    let (native, revision) = mcode_config::snapshot()?;
    if expected != revision {
        return Err(changed());
    }
    let live = native.get(&provider.id);
    if live.is_none() && mcode_config::provider_key_exists(&provider.id)? {
        return Err(AppError::InvalidInput(
            "MCode owns this account provider".into(),
        ));
    }
    preserve_unknown_fields(
        live.unwrap_or(&previous.settings_config),
        &mut provider.settings_config,
    );
    // Blank key on edit means retain the existing credential, never erase it.
    let keep_key = match provider.settings_config.pointer("/options/apiKey") {
        None => true,
        Some(Value::String(key)) => key.trim().is_empty(),
        Some(_) => {
            return Err(AppError::InvalidInput(
                "MCode API key must be a string".into(),
            ))
        }
    };
    if keep_key {
        if let Some(key) = live
            .unwrap_or(&previous.settings_config)
            .pointer("/options/apiKey")
        {
            if let Some(options) = provider
                .settings_config
                .get_mut("options")
                .and_then(Value::as_object_mut)
            {
                options.insert("apiKey".into(), key.clone());
            }
        }
    }
    write_name(&mut provider);
    prepare(&mut provider, live.is_some())?;
    if live.is_some() {
        mcode_config::transact_at(
            &mcode_config::config_path(),
            &provider.id,
            Some(provider.settings_config.clone()),
            false,
            Some(&expected),
            |receipt| create_backup(state, receipt),
            || save_user_provider(state, &provider),
        )?;
    } else {
        if mcode_config::current_revision()? != expected {
            return Err(changed());
        }
        save_user_provider(state, &provider)?;
    }
    Ok(true)
}

pub(super) fn enable(state: &AppState, id: &str) -> Result<SwitchResult, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(APP));
    let mut provider = saved_provider(state, id)?;
    let (native, revision) = mcode_config::snapshot()?;
    require_unchanged_node(native.get(id), &provider)?;
    prepare(&mut provider, true)?;
    mcode_config::transact_at(
        &mcode_config::config_path(),
        id,
        Some(provider.settings_config.clone()),
        !native.contains_key(id),
        Some(&revision),
        |receipt| create_backup(state, receipt),
        || save_user_provider(state, &provider),
    )?;
    Ok(SwitchResult::default())
}

pub(super) fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
    remove_inner(state, id, false)
}
pub(super) fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
    remove_inner(state, id, true)
}
fn remove_inner(state: &AppState, id: &str, delete: bool) -> Result<(), AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(APP));
    let mut provider = saved_provider(state, id)?;
    let (native, revision) = mcode_config::snapshot()?;
    require_unchanged_node(native.get(id), &provider)?;
    // Do not normalize/validate stale catalog entries merely to allow their removal.
    let meta = provider.meta.get_or_insert_with(ProviderMeta::default);
    meta.live_config_managed = Some(false);
    meta.mcode_config_revision = None;
    let commit = || {
        if delete {
            state.db.delete_provider(APP, id)
        } else {
            state.db.save_provider(APP, &provider)
        }
    };
    if native.contains_key(id) {
        mcode_config::transact_at(
            &mcode_config::config_path(),
            id,
            None,
            false,
            Some(&revision),
            |receipt| create_backup(state, receipt),
            commit,
        )
    } else {
        // An excluded account-owned node is never removed, even on catalog deletion.
        commit()
    }
}

fn save_user_provider(state: &AppState, provider: &Provider) -> Result<(), AppError> {
    let vault = state.db.secrets.read()?;
    state
        .db
        .save_provider_with_vault(APP, provider, None, true, state.db.secrets.as_ref(), &vault)
}
fn saved_provider(state: &AppState, id: &str) -> Result<Provider, AppError> {
    state
        .db
        .get_provider_by_id(id, APP)?
        .ok_or_else(|| AppError::InvalidInput("MCode provider not found".into()))
}
fn changed() -> AppError {
    AppError::Conflict("MCode configuration changed; refresh and review before saving".into())
}
fn require_unchanged_node(native: Option<&Value>, provider: &Provider) -> Result<(), AppError> {
    if native.is_some_and(|config| config != &provider.settings_config) {
        return Err(AppError::Conflict(
            "MCode provider changed externally; explicitly import it before adding or removing"
                .into(),
        ));
    }
    Ok(())
}
fn prepare(provider: &mut Provider, live: bool) -> Result<(), AppError> {
    if crate::relay::is_managed(&provider.id) || provider.uses_managed_account_auth() {
        return Err(AppError::InvalidInput(
            "MCode accepts custom API providers, not managed accounts".into(),
        ));
    }
    mcode_config::validate_provider(&provider.id, &provider.settings_config)?;
    // These application-native entries do not own proxy, usage or account metadata.
    provider.meta = Some(ProviderMeta {
        live_config_managed: Some(live),
        ..Default::default()
    });
    provider.in_failover_queue = false;
    provider.available_models = None;
    Ok(())
}
fn write_name(provider: &mut Provider) {
    if let Some(object) = provider.settings_config.as_object_mut() {
        object.insert("name".into(), Value::String(provider.name.clone()));
    }
}
fn align_name(provider: &mut Provider) {
    if let Some(name) = provider.settings_config.get("name").and_then(Value::as_str) {
        provider.name = name.to_string();
    }
}

/// Only top-level extensions are outside this editor's ownership. Options and
/// model objects are fully editable (including their extra fields), so their
/// submitted shape must retain explicit deletions, not recursively fill them in.
fn preserve_unknown_fields(previous: &Value, submitted: &mut Value) {
    let (Some(old), Some(new)) = (previous.as_object(), submitted.as_object_mut()) else {
        return;
    };
    for (key, value) in old {
        if matches!(
            key.as_str(),
            "kind" | "name" | "api" | "enabled" | "options" | "models"
        ) {
            continue;
        }
        new.entry(key.clone()).or_insert_with(|| value.clone());
    }
}

#[derive(serde::Serialize)]
pub(crate) struct BackupDescriptor {
    pub id: String,
    pub revision: String,
}
fn backup_file(id: &str) -> Result<OwnedFile, AppError> {
    let path = Path::new(id);
    if path.components().count() != 1 {
        return Err(AppError::InvalidInput("Invalid MCode backup id".into()));
    }
    OwnedFile::registered(Path::new("backups/mcode").join(path))
}
fn create_backup(state: &AppState, receipt: &ConfigBackup) -> Result<(), AppError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| AppError::Config("Invalid backup time".into()))?
        .as_nanos();
    let id = format!("mcode_{nanos:030}_{}.json", uuid::Uuid::new_v4().as_u128());
    let bytes =
        serde_json::to_vec(receipt).map_err(|_| AppError::Config("Invalid MCode backup".into()))?;
    backup_file(&id)?.write(state.db.secrets.as_ref(), &bytes)
}
fn read_backup(state: &AppState, id: &str) -> Result<ConfigBackup, AppError> {
    let file = backup_file(id)?;
    let path = state.db.secrets.root().join(file.relative_path());
    let encrypted = crate::config_file_io::read_regular_file(&path, 32 * 1024 * 1024)
        .map_err(|error| AppError::io(&path, error))?
        .ok_or_else(|| AppError::InvalidInput("MCode backup not found".into()))?;
    let vault = state.db.secrets.read()?;
    let bytes = file.decode(&vault, &encrypted)?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::Config("Invalid MCode backup".into()))
}
pub(crate) fn latest_backup(state: &AppState) -> Result<Option<BackupDescriptor>, AppError> {
    let directory = state.db.secrets.root().join("backups/mcode");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(AppError::io(&directory, error)),
    };
    let mut ids = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|id| backup_file(id).is_ok())
        .collect::<Vec<_>>();
    ids.sort();
    if ids.is_empty() {
        return Ok(None);
    }
    // A failed transaction may have left a newer receipt for bytes it rolled
    // back. Do not let that receipt hide an older, still-applicable backup.
    // Preserve secret-session availability errors instead of reporting no backups.
    {
        let _vault = state.db.secrets.read()?;
    }
    let revision = mcode_config::current_revision()?;
    let target = mcode_config::config_path();
    for id in ids.into_iter().rev() {
        let Ok(receipt) = read_backup(state, &id) else {
            // A concurrent credential lifecycle transition is not a corrupt file.
            let _vault = state.db.secrets.read()?;
            continue;
        };
        if receipt.version == 1 && receipt.target == target && receipt.after_revision == revision {
            return Ok(Some(BackupDescriptor { id, revision }));
        }
    }
    Ok(None)
}
pub(crate) fn restore_backup(state: &AppState, id: &str, revision: &str) -> Result<bool, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(APP));
    let receipt = read_backup(state, id)?;
    mcode_config::restore_at(
        &mcode_config::config_path(),
        &receipt,
        revision,
        |undo| create_backup(state, undo),
        || Ok(()),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn editor_owned_options_and_model_fields_respect_explicit_deletion() {
        let old = json!({"opaque":{"keep":true},"options":{"apiKey":"fixture", "headers":{"X-Old":"remove"},"future":9,"timeout":1200},"models":{"keep":{"future":7,"limit":{"context":100},"options":{"reasoningEffort":"high"}},"remove":{}}});
        let mut next = json!({"options":{"apiKey":"replacement"},"models":{"keep":{"name":"New"}}});
        preserve_unknown_fields(&old, &mut next);
        assert_eq!(next["opaque"], old["opaque"]);
        assert!(next["options"].get("future").is_none());
        assert!(next["options"].get("timeout").is_none());
        assert!(next["options"].get("headers").is_none());
        assert!(next["models"].get("remove").is_none());
        assert!(next["models"]["keep"].get("future").is_none());
        assert!(next["models"]["keep"].get("limit").is_none());
        assert!(next["models"]["keep"].get("options").is_none());
        assert_eq!(next["options"]["apiKey"], "replacement");
    }
    #[test]
    fn backup_ids_are_registry_scoped_and_cannot_escape() {
        assert!(backup_file("mcode_123_456.json").is_ok());
        for id in [
            "../mcode_123.json",
            "/mcode_123.json",
            "other_123.json",
            "mcode_abc.json",
        ] {
            assert!(backup_file(id).is_err());
        }
    }
    #[test]
    fn imported_native_shape_does_not_gain_a_name_or_revision() {
        let config = json!({"kind":"custom","options":{"baseURL":"https://example.test","apiKey":"fixture"},"models":{"m":{}}});
        let mut provider =
            Provider::with_id("custom".into(), "Custom".into(), config.clone(), None);
        prepare(&mut provider, true).unwrap();
        assert_eq!(provider.settings_config, config);
        assert!(provider.meta.unwrap().mcode_config_revision.is_none());
    }
}
