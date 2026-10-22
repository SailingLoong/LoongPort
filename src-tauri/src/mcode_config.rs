//! MiniMax Code native custom providers. Adapted from cc-switch v4.0.18 (MIT).
//! Copyright (c) 2025 Jason Young. Upstream module remains the configuration owner.
//! LoongPort: bounded reads, revision checks, encrypted-backup hook and locked rollback.
//! Native login, non-custom providers and model selection remain owned by MiniMax Code.
use crate::config::{atomic_write_private, get_home_dir};
use crate::error::AppError;
use indexmap::IndexMap;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn data_dir() -> PathBuf {
    explicit_data_dir(
        std::env::var("MINIMAX_DATA_DIR").ok().as_deref(),
        std::env::var("MAVIS_DATA_DIR").ok().as_deref(),
    )
    .unwrap_or_else(|| get_home_dir().join(".minimax"))
}

fn explicit_data_dir(minimax: Option<&str>, mavis: Option<&str>) -> Option<PathBuf> {
    [minimax, mavis]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|path| !path.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn config_path() -> PathBuf {
    data_dir().join("config.yaml")
}

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    if !path.is_absolute() {
        return Err(AppError::Config(
            "MCode configuration path must be absolute".into(),
        ));
    }
    crate::config_file_io::read_regular_file(path, MAX_CONFIG_BYTES)
        .map_err(|e| AppError::io(path, e))
}
fn parse(bytes: Option<&[u8]>) -> Result<serde_yaml::Value, AppError> {
    let value: serde_yaml::Value = serde_yaml::from_slice(bytes.unwrap_or(b""))
        .map_err(|_| AppError::Config("Invalid MCode YAML configuration; file preserved".into()))?;
    if value.is_null() {
        return Ok(serde_yaml::Value::Mapping(Default::default()));
    }
    if !value.is_mapping() {
        return Err(AppError::Config(
            "MCode configuration must be a mapping".into(),
        ));
    }
    if value
        .get("custom_provider")
        .is_some_and(|p| !p.is_null() && !p.is_mapping())
    {
        return Err(AppError::Config(
            "Invalid MCode custom_provider configuration".into(),
        ));
    }
    Ok(value)
}
fn read(path: &Path) -> Result<serde_yaml::Value, AppError> {
    parse(read_bytes(path)?.as_deref())
}
pub(crate) fn revision(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|v| hex::encode(Sha256::digest(v)))
        .unwrap_or_else(|| "missing".into())
}
pub(crate) fn current_revision() -> Result<String, AppError> {
    Ok(revision(read_bytes(&config_path())?.as_deref()))
}

pub(crate) fn get_providers() -> Result<IndexMap<String, Value>, AppError> {
    snapshot().map(|(providers, _)| providers)
}

pub(crate) fn snapshot() -> Result<(IndexMap<String, Value>, String), AppError> {
    let bytes = read_bytes(&config_path())?;
    let document = parse(bytes.as_deref())?;
    let providers = providers_from(&document)?;
    Ok((providers, revision(bytes.as_deref())))
}

fn providers_from(document: &serde_yaml::Value) -> Result<IndexMap<String, Value>, AppError> {
    match document.get("custom_provider") {
        None | Some(serde_yaml::Value::Null) => Ok(IndexMap::new()),
        Some(value) => {
            let mut providers: IndexMap<String, Value> = serde_yaml::from_value(value.clone())
                .map_err(|_| {
                    AppError::Config("Invalid MCode custom_provider configuration".into())
                })?;
            providers.retain(|_, provider| {
                provider.is_object() && provider.get("kind").is_none_or(|kind| kind == "custom")
            });
            Ok(providers)
        }
    }
}

/// Native default-model references are presentation facts, never LoongPort selections.
pub(crate) fn referenced_provider_ids() -> Result<std::collections::HashSet<String>, AppError> {
    let document = read(&config_path())?;
    Ok(referenced_provider_ids_from(&document))
}

fn referenced_provider_ids_from(document: &serde_yaml::Value) -> std::collections::HashSet<String> {
    ["defaultModel", "defaultLightModel"]
        .into_iter()
        .filter_map(|field| document.get(field).and_then(serde_yaml::Value::as_str))
        .filter_map(|model| model.strip_prefix("custom_provider:"))
        .filter_map(|reference| reference.split_once('/'))
        .filter(|(id, model)| !id.is_empty() && !model.is_empty())
        .map(|(id, _)| id.to_string())
        .collect()
}

/// Any `custom_provider` node under this key, including MCode's own account providers.
pub(crate) fn provider_key_exists(id: &str) -> Result<bool, AppError> {
    let document = read(&config_path())?;
    Ok(document
        .get("custom_provider")
        .and_then(serde_yaml::Value::as_mapping)
        .is_some_and(|providers| providers.contains_key(serde_yaml::Value::from(id))))
}

pub(crate) fn validate_provider(id: &str, config: &Value) -> Result<(), AppError> {
    if !config.is_object() || config.get("kind").is_some_and(|kind| kind != "custom") {
        return Err(AppError::InvalidInput(
            "MCode owns this account provider".into(),
        ));
    }
    if id.is_empty()
        || id
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_'))
    {
        return Err(AppError::InvalidInput(
            "MCode provider key must contain letters, digits, '-' or '_'".into(),
        ));
    }
    if !matches!(
        config
            .get("api")
            .map_or(Some("anthropic-messages"), Value::as_str),
        Some("anthropic-messages" | "openai-completions" | "openai-responses")
    ) {
        return Err(AppError::InvalidInput(
            "Select a supported MCode API format".into(),
        ));
    }
    let base_url = config
        .pointer("/options/baseURL")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !url::Url::parse(base_url).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    }) {
        return Err(AppError::InvalidInput(
            "Enter a valid MCode endpoint URL".into(),
        ));
    }
    if config
        .pointer("/options/apiKey")
        .and_then(Value::as_str)
        .is_none_or(|key| key.trim().is_empty())
    {
        return Err(AppError::InvalidInput("Enter an API key".into()));
    }
    if config
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(AppError::InvalidInput(
            "MCode enabled must be a boolean".into(),
        ));
    }
    if config.pointer("/options/headers").is_some_and(|headers| {
        headers
            .as_object()
            .is_none_or(|headers| headers.values().any(|value| !value.is_string()))
    }) {
        return Err(AppError::InvalidInput(
            "MCode headers must be a string map".into(),
        ));
    }
    if config
        .get("models")
        .and_then(Value::as_object)
        .is_none_or(|models| {
            models.is_empty()
                || models.iter().any(|(id, model)| {
                    id.trim().is_empty()
                        || !model.is_object()
                        || model
                            .get("enabled")
                            .is_some_and(|value| !value.is_boolean())
                })
        })
    {
        return Err(AppError::InvalidInput(
            "Add valid MCode model objects with boolean enabled flags".into(),
        ));
    }
    Ok(())
}

// MCode's proper-lockfile uses the same atomic directory lock.
struct ConfigLock(PathBuf);
impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn lock(path: &Path) -> Result<ConfigLock, AppError> {
    if !path.is_absolute() {
        return Err(AppError::Config(
            "MCode configuration path must be absolute".into(),
        ));
    }
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| AppError::Config("Invalid MCode path".into()))?,
    )
    .map_err(|e| AppError::io(path, e))?;
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    fs::create_dir(&lock_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            AppError::Conflict(
                "MCode configuration is busy; close the native writer and retry".into(),
            )
        } else {
            AppError::io(&lock_path, error)
        }
    })?;
    Ok(ConfigLock(lock_path))
}

/// Native lock spans read, backup, atomic publication and database commit/rollback.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigBackup {
    pub version: u8,
    pub target: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after_revision: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn transact_at<T>(
    path: &Path,
    id: &str,
    provider: Option<Value>,
    create_only: bool,
    expected_revision: Option<&str>,
    backup: impl FnOnce(&ConfigBackup) -> Result<(), AppError>,
    commit: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let _lock = lock(path)?;
    let previous = read_bytes(path)?;
    if expected_revision.is_some_and(|expected| expected != revision(previous.as_deref())) {
        return Err(AppError::Conflict(
            "MCode configuration changed; refresh and review before saving".into(),
        ));
    }
    let mut document = parse(previous.as_deref())?;

    let prefix = format!("custom_provider:{id}/");
    if ["defaultModel", "defaultLightModel"].iter().any(|field| {
        let selected = document
            .get(*field)
            .and_then(serde_yaml::Value::as_str)
            .and_then(|model| model.strip_prefix(&prefix));
        selected.is_some_and(|model| {
            !provider.as_ref().is_some_and(|provider| {
                provider.get("enabled") != Some(&Value::Bool(false))
                    && provider
                        .get("models")
                        .and_then(|models| models.get(model))
                        .is_some_and(|model| {
                            model.is_object() && model.get("enabled") != Some(&Value::Bool(false))
                        })
            })
        })
    }) {
        return Err(AppError::InvalidInput(
            "Select another default model in MCode before removing this model or provider".into(),
        ));
    }
    let root = document.as_mapping_mut().expect("Validated mapping");
    let entry = root
        .entry("custom_provider".into())
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    if entry.is_null() {
        *entry = serde_yaml::Value::Mapping(Default::default());
    }
    let providers = entry
        .as_mapping_mut()
        .ok_or_else(|| AppError::Config("Invalid MCode custom_provider configuration".into()))?;
    if create_only && providers.contains_key(serde_yaml::Value::from(id)) {
        return Err(AppError::InvalidInput(format!(
            "MCode provider key '{id}' already exists"
        )));
    }
    if providers
        .get(serde_yaml::Value::from(id))
        .is_some_and(|value| {
            !value.is_mapping()
                || value
                    .get("kind")
                    .is_some_and(|kind| kind.as_str() != Some("custom"))
        })
    {
        return Err(AppError::InvalidInput(
            "MCode owns this account provider".into(),
        ));
    }
    if let Some(provider) = provider {
        providers.insert(
            id.into(),
            serde_yaml::to_value(provider)
                .map_err(|_| AppError::Config("Invalid MCode provider".into()))?,
        );
    } else {
        providers.remove(serde_yaml::Value::from(id));
    }
    let yaml = serde_yaml::to_string(&document)
        .map_err(|_| AppError::Config("Cannot serialize MCode configuration".into()))?;
    if yaml.len() as u64 > MAX_CONFIG_BYTES {
        return Err(AppError::InvalidInput(
            "MCode configuration exceeds the safe size limit".into(),
        ));
    }
    backup(&ConfigBackup {
        version: 1,
        target: path.to_path_buf(),
        before: previous.clone(),
        after_revision: revision(Some(yaml.as_bytes())),
    })?;
    if read_bytes(path)? != previous {
        return Err(AppError::Conflict(
            "MCode configuration changed before write".into(),
        ));
    }
    atomic_write_private(path, yaml.as_bytes())?;
    match commit() {
        Ok(result) => Ok(result),
        Err(error) => {
            if read_bytes(path)?.as_deref() != Some(yaml.as_bytes()) {
                return Err(AppError::Conflict(format!("MCode commit failed ({error}); newer native changes preserved; restore the encrypted backup after review")));
            }
            let rollback = match previous {
                Some(bytes) => atomic_write_private(path, &bytes),
                None => fs::remove_file(path).map_err(|e| AppError::io(path, e)),
            };
            rollback.map_err(|rollback|AppError::Config(format!("MCode commit failed ({error}); rollback failed ({rollback}); encrypted backup retained")))?;
            Err(error)
        }
    }
}

pub(crate) fn restore_at<T>(
    path: &Path,
    receipt: &ConfigBackup,
    expected_revision: &str,
    backup: impl FnOnce(&ConfigBackup) -> Result<(), AppError>,
    commit: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    if receipt.version != 1 || receipt.target != path {
        return Err(AppError::InvalidInput(
            "MCode backup belongs to another configuration".into(),
        ));
    }
    if receipt
        .before
        .as_ref()
        .is_some_and(|bytes| bytes.len() as u64 > MAX_CONFIG_BYTES)
    {
        return Err(AppError::Config("MCode backup is too large".into()));
    }
    parse(receipt.before.as_deref())?;
    let _lock = lock(path)?;
    let current = read_bytes(path)?;
    let current_revision = revision(current.as_deref());
    if current_revision != expected_revision || current_revision != receipt.after_revision {
        return Err(AppError::Conflict(
            "MCode configuration changed since this backup; newer state preserved".into(),
        ));
    }
    backup(&ConfigBackup {
        version: 1,
        target: path.to_path_buf(),
        before: current.clone(),
        after_revision: revision(receipt.before.as_deref()),
    })?;
    if read_bytes(path)? != current {
        return Err(AppError::Conflict(
            "MCode configuration changed before restore".into(),
        ));
    }
    match &receipt.before {
        Some(bytes) => atomic_write_private(path, bytes)?,
        None => {
            if current.is_some() {
                fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
            }
        }
    }
    match commit() {
        Ok(result) => Ok(result),
        Err(error) => {
            if read_bytes(path)? != receipt.before {
                return Err(AppError::Conflict(format!(
                    "MCode restore commit failed ({error}); newer native changes preserved"
                )));
            }
            match current {
                Some(bytes) => atomic_write_private(path, &bytes)?,
                None => {
                    if path.exists() {
                        fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
                    }
                }
            }
            Err(error)
        }
    }
}

#[cfg(test)]
#[path = "mcode_config_tests.rs"]
mod tests;
