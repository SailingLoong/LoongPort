//! ZCode's personal provider file is the sole owner of these configurations.
//! Contract: zai-org/ZCode 29628c9a, provider-node/file-codec and shared/node/atomicFileLock.
//! Keep account credentials, defaults and unrelated rules native-owned.

use crate::config::{atomic_write_private, get_home_dir};
use crate::error::AppError;
#[cfg(test)]
use crate::zcode_file_lock::now_ms;
use crate::zcode_file_lock::{unique_token, FileLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MANAGED_PREFIX: &str = "loongport-";
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodeProviderView {
    pub id: String,
    pub name: String,
    pub api_type: String,
    pub base_url: String,
    pub models: Vec<String>,
    pub has_api_key: bool,
    pub managed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodeConfigView {
    pub revision: String,
    pub providers: Vec<ZCodeProviderView>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ZCodeProviderInput {
    pub id: Option<String>,
    pub revision: String,
    pub name: String,
    pub api_type: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub models: Vec<String>,
}

fn config_error(message: &str) -> AppError {
    AppError::Config(format!("ZCode: {message}"))
}

pub(crate) fn resolve_path(
    home: &Path,
    base: Option<&str>,
    file: Option<&str>,
) -> Result<PathBuf, AppError> {
    let path = if let Some(file) = file.filter(|s| !s.trim().is_empty()) {
        PathBuf::from(file.trim())
    } else {
        base.filter(|s| !s.trim().is_empty())
            .map(|s| PathBuf::from(s.trim()))
            .unwrap_or_else(|| home.to_path_buf())
            .join(".zcode/v2/provider_config.json")
    };
    if !path.is_absolute() {
        return Err(config_error("configuration path must be absolute"));
    }
    Ok(path)
}

pub(crate) fn config_path() -> Result<PathBuf, AppError> {
    resolve_path(
        &get_home_dir(),
        std::env::var("ZCODE_DATA_BASE_DIR").ok().as_deref(),
        std::env::var("ZCODE_PERSONAL_PROVIDER_CONFIG_FILE")
            .ok()
            .as_deref(),
    )
}

pub(crate) fn read() -> Result<ZCodeConfigView, AppError> {
    read_at(&config_path()?)
}
pub(crate) fn save(input: ZCodeProviderInput) -> Result<ZCodeConfigView, AppError> {
    save_at(&config_path()?, input)
}
pub(crate) fn remove(id: &str, revision: &str) -> Result<ZCodeConfigView, AppError> {
    remove_at(&config_path()?, id, revision)
}

fn empty_document() -> Value {
    json!({"schemaVersion":1,"config":{"providerConfigRules":{"providerRules":[]},
        "modelConfigRules":{"providerModelRules":[],"manualProviderModelRules":[]}}})
}

fn read_document(path: &Path) -> Result<(Value, String), AppError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((empty_document(), "missing".into()))
        }
        Err(_) => return Err(config_error("cannot read personal configuration")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| config_error("cannot read personal configuration"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(config_error("personal configuration is too large"));
    }
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|_| config_error("invalid personal configuration JSON; file preserved"))?;
    validate_document(&document)?;
    Ok((document, hex::encode(Sha256::digest(&bytes))))
}

fn validate_document(doc: &Value) -> Result<(), AppError> {
    if doc.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(config_error(
            "unsupported personal configuration schema; file preserved",
        ));
    }
    let config = doc
        .get("config")
        .and_then(Value::as_object)
        .ok_or_else(|| config_error("invalid config object"))?;
    let rules = config
        .get("providerConfigRules")
        .and_then(|v| v.get("providerRules"))
        .and_then(Value::as_array)
        .ok_or_else(|| config_error("invalid provider rules"))?;
    let mut ids = std::collections::HashSet::new();
    for rule in rules {
        let id = rule
            .get("providerId")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| config_error("invalid provider identity"))?;
        if !ids.insert(id) || !rule.get("config").is_some_and(Value::is_object) {
            return Err(config_error("invalid or duplicate provider rule"));
        }
    }
    for key in ["providerModelRules", "manualProviderModelRules"] {
        if !config
            .get("modelConfigRules")
            .and_then(|v| v.get(key))
            .is_some_and(Value::is_array)
        {
            return Err(config_error("invalid model rules"));
        }
    }
    if config.get("providerOrder").is_some_and(|v| {
        !v.is_array()
            || v.as_array()
                .is_some_and(|a| a.iter().any(|v| !v.is_string()))
    }) {
        return Err(config_error("invalid provider order"));
    }
    Ok(())
}

// The backend owns editability; prefixes alone cannot grant control over native templates.
fn can_manage(rule: &Value) -> bool {
    rule["providerId"]
        .as_str()
        .is_some_and(|id| id.starts_with(MANAGED_PREFIX))
        && rule["config"]["access"]["type"].as_str() == Some("api-key")
        && rule["config"]["api"]["baseUrl"]
            .as_str()
            .is_some_and(|url| validate_api_url(url).is_ok())
        && rule.get("templateId").is_none_or(Value::is_null)
        && rule["config"]
            .get("group")
            .is_none_or(|group| group == "standard-personal")
}

fn view(document: &Value, revision: String) -> ZCodeConfigView {
    let providers = document["config"]["providerConfigRules"]["providerRules"]
        .as_array()
        .unwrap()
        .iter()
        // Account access stays entirely inside ZCode. Only ordinary personal providers are displayed.
        .filter(|r| !r["providerId"].as_str().unwrap().starts_with("account:"))
        .map(|r| {
            let config = &r["config"];
            let id = r["providerId"].as_str().unwrap().to_string();
            ZCodeProviderView {
                name: r["providerName"].as_str().unwrap_or(&id).to_string(),
                managed: can_manage(r),
                id,
                api_type: config["api"]["type"].as_str().unwrap_or_default().into(),
                base_url: config["api"]["baseUrl"]
                    .as_str()
                    .filter(|url| validate_api_url(url).is_ok())
                    .unwrap_or_default()
                    .into(),
                models: config["personalModelIds"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                has_api_key: config["access"]["apiKey"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty()),
            }
        })
        .collect();
    ZCodeConfigView {
        revision,
        providers,
    }
}

pub(crate) fn read_at(path: &Path) -> Result<ZCodeConfigView, AppError> {
    let (doc, revision) = read_document(path)?;
    Ok(view(&doc, revision))
}

// The same boundary controls input acceptance, native editability and redacted views.
fn validate_api_url(base_url: &str) -> Result<(), AppError> {
    let url = url::Url::parse(base_url.trim()).map_err(|_| config_error("invalid API URL"))?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(config_error(
            "API URL must be HTTP(S), without credentials, query or fragment",
        ));
    }
    Ok(())
}

fn validate_input(input: &ZCodeProviderInput) -> Result<(), AppError> {
    if input.name.trim().is_empty() || input.name.len() > 256 {
        return Err(config_error("provider name is required"));
    }
    if ![
        "anthropic-messages",
        "openai-chat-completions",
        "openai-responses",
    ]
    .contains(&input.api_type.as_str())
    {
        return Err(config_error("unsupported API protocol"));
    }
    validate_api_url(&input.base_url)?;
    if input.models.is_empty()
        || input.models.len() > 256
        || input
            .models
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 512)
    {
        return Err(config_error("at least one valid model ID is required"));
    }
    if let Some(id) = &input.id {
        validate_owned_id(id)?;
    }
    Ok(())
}

fn validate_owned_id(id: &str) -> Result<(), AppError> {
    if !id.starts_with(MANAGED_PREFIX) || id.len() > 256 {
        return Err(config_error("only LoongPort providers can be changed"));
    }
    Ok(())
}

pub(crate) fn save_at(path: &Path, input: ZCodeProviderInput) -> Result<ZCodeConfigView, AppError> {
    validate_input(&input)?;
    let _lock = FileLock::acquire(path, LOCK_TIMEOUT)?;
    let (mut doc, revision) = read_document(path)?;
    if revision != input.revision {
        return Err(AppError::Conflict(
            "ZCode configuration changed; refresh before saving".into(),
        ));
    }
    let rules = doc["config"]["providerConfigRules"]["providerRules"]
        .as_array_mut()
        .unwrap();
    let mut rule = if let Some(id) = &input.id {
        rules
            .iter()
            .find(|r| r["providerId"].as_str() == Some(id))
            .cloned()
            .ok_or_else(|| config_error("provider no longer exists"))?
    } else {
        json!({"providerId":format!("{MANAGED_PREFIX}{}",unique_token()),"providerName":input.name,"config":{"group":"standard-personal","access":{"type":"api-key"},"api":{}}})
    };
    // Do not take over an account or native template even if its ID uses our prefix.
    if input.id.is_some() && !can_manage(&rule) {
        return Err(config_error(
            "only standalone API Key providers can be changed",
        ));
    }
    if let Some(key) = input.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        rule["config"]["access"]["apiKey"] = json!(key.trim());
    }
    if rule["config"]["access"]["apiKey"]
        .as_str()
        .is_none_or(|s| s.trim().is_empty())
    {
        return Err(config_error("API Key is required"));
    }
    let mut models = Vec::new();
    for model in &input.models {
        let model = model.trim().to_string();
        if !models.contains(&model) {
            models.push(model);
        }
    }
    rule["providerName"] = json!(input.name.trim());
    rule["config"]["api"]["type"] = json!(input.api_type);
    rule["config"]["api"]["baseUrl"] = json!(input.base_url.trim());
    rule["config"]["personalModelIds"] = json!(models);
    let id = rule["providerId"].as_str().unwrap().to_string();
    if let Some(slot) = rules
        .iter_mut()
        .find(|r| r["providerId"].as_str() == Some(&id))
    {
        *slot = rule;
    } else {
        rules.push(rule);
    }
    write_document(path, &doc)?;
    read_at(path)
}

pub(crate) fn remove_at(
    path: &Path,
    id: &str,
    expected_revision: &str,
) -> Result<ZCodeConfigView, AppError> {
    validate_owned_id(id)?;
    let _lock = FileLock::acquire(path, LOCK_TIMEOUT)?;
    let (mut doc, revision) = read_document(path)?;
    if revision != expected_revision {
        return Err(AppError::Conflict(
            "ZCode configuration changed; refresh before removing".into(),
        ));
    }
    let rules = doc["config"]["providerConfigRules"]["providerRules"]
        .as_array_mut()
        .unwrap();
    let rule = rules
        .iter()
        .find(|r| r["providerId"].as_str() == Some(id))
        .ok_or_else(|| config_error("provider no longer exists"))?;
    if !can_manage(rule) {
        return Err(config_error(
            "only standalone API Key providers can be removed",
        ));
    }
    rules.retain(|r| r["providerId"].as_str() != Some(id));
    for key in ["providerModelRules", "manualProviderModelRules"] {
        doc["config"]["modelConfigRules"][key]
            .as_array_mut()
            .unwrap()
            .retain(|r| r["providerId"].as_str() != Some(id));
    }
    if let Some(order) = doc["config"]
        .get_mut("providerOrder")
        .and_then(Value::as_array_mut)
    {
        order.retain(|v| v.as_str() != Some(id));
    }
    // A user's default selection remains their intent, even when its provider is removed.
    write_document(path, &doc)?;
    read_at(path)
}

fn write_document(path: &Path, doc: &Value) -> Result<(), AppError> {
    validate_document(doc)?;
    let bytes = serde_json::to_vec_pretty(doc)
        .map_err(|_| config_error("cannot serialize configuration"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(config_error(
            "personal configuration is too large; file preserved",
        ));
    }
    atomic_write_private(path, &bytes)
}

#[cfg(test)]
#[path = "zcode_config_tests.rs"]
mod tests;
