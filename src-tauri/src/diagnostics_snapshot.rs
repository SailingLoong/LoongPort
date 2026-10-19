//! On-demand diagnostic projections. Only explicit safe fields enter a manifest.
//! This module never serializes a config, provider, tool error, or catalog row.
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_CATALOG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CollectionStatus {
    Ok,
    NotConfigured,
    NotSupported,
    NotObserved,
    Unavailable,
    Timeout,
    Truncated,
}

pub(crate) fn status(status: CollectionStatus) -> Value {
    json!({"status": status})
}

fn collected(data: Value) -> Value {
    json!({"status": CollectionStatus::Ok, "data": data})
}

/// The closure is not invoked without consent: no disk reads or CLI probes.
/// These four basic facts are the only feedback metadata sent without diagnostics.
pub(crate) fn environment_manifest(
    include_diagnostics: bool,
    collect: impl FnOnce() -> Value,
) -> Value {
    let mut manifest = json!({
        "appVersion": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
    });
    if include_diagnostics {
        manifest["diagnostics"] = collect();
    }
    manifest
}

fn io_status(error: std::io::Error) -> CollectionStatus {
    match error.kind() {
        std::io::ErrorKind::NotFound => CollectionStatus::NotConfigured,
        std::io::ErrorKind::TimedOut => CollectionStatus::Timeout,
        _ => CollectionStatus::Unavailable,
    }
}

/// Open without following the final symlink, then inspect and read that same
/// handle. There is no metadata(path)-then-open(path) check/use window.
fn read_bounded(path: &Path, max_bytes: u64) -> Result<String, CollectionStatus> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // NONBLOCK prevents a replaced FIFO from hanging open before fstat.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    return Err(CollectionStatus::NotSupported);

    let file = options.open(path).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return CollectionStatus::NotSupported;
        }
        io_status(error)
    })?;
    read_open_file_bounded(file, max_bytes)
}

fn read_open_file_bounded(file: std::fs::File, max_bytes: u64) -> Result<String, CollectionStatus> {
    let metadata = file.metadata().map_err(io_status)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(CollectionStatus::NotSupported);
        }
    }
    if !metadata.is_file() {
        return Err(CollectionStatus::NotSupported);
    }
    if metadata.len() > max_bytes {
        return Err(CollectionStatus::Truncated);
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(io_status)?;
    if bytes.len() as u64 > max_bytes {
        return Err(CollectionStatus::Truncated);
    }
    String::from_utf8(bytes).map_err(|_| CollectionStatus::Unavailable)
}

fn enum_field(value: Option<&str>, allowed: &[&str], present: bool) -> Value {
    match value {
        Some(value) if allowed.contains(&value) => json!({"status":"ok", "value":value}),
        _ if present => status(CollectionStatus::Unavailable),
        _ => status(CollectionStatus::NotConfigured),
    }
}

fn number_field(value: Option<u64>, present: bool) -> Value {
    match value.filter(|v| *v > 0) {
        Some(value) => json!({"status":"ok", "value":value}),
        None if present => status(CollectionStatus::Unavailable),
        None => status(CollectionStatus::NotConfigured),
    }
}

fn config_number(doc: &toml_edit::DocumentMut, key: &str) -> Value {
    let item = doc.get(key);
    number_field(
        item.and_then(|v| v.as_integer())
            .and_then(|v| v.try_into().ok()),
        item.is_some(),
    )
}

fn catalog_number(entry: &Value, key: &str) -> Value {
    number_field(
        entry.get(key).and_then(Value::as_u64),
        entry.get(key).is_some(),
    )
}

/// No capability inference here: the existing native catalog owner decides it.
/// In particular, a missing or empty list must never become "unsupported".
fn reasoning_projection(
    capability: Option<crate::codex_reasoning::ReasoningCapabilities>,
) -> Value {
    let capability = capability.unwrap_or_default();
    json!({
        "mode": capability.mode,
        "levels": capability.levels,
        "defaultLevel": capability.default_level,
        "source": capability.source,
    })
}

/// Only an exact canonical slug in the versioned public official seed may be
/// shown. A familiar prefix or model family never declassifies a private ID.
fn public_model_id(model: &str) -> Option<&str> {
    crate::codex_reasoning::seed_official_model_facts()
        .iter()
        .any(|entry| entry.get("slug").and_then(Value::as_str) == Some(model))
        .then_some(model)
}

fn selected_catalog_snapshot(
    config_text: &str,
    config_path: &Path,
    model: Option<&str>,
    doc: &toml_edit::DocumentMut,
) -> Value {
    let reference = doc.get("model_catalog_json");
    if reference.is_none() {
        return status(CollectionStatus::NotConfigured);
    }
    if reference
        .and_then(|v| v.as_str())
        .is_none_or(|v| v.trim().is_empty())
    {
        return status(CollectionStatus::Unavailable);
    }
    let Some(base) = config_path.parent() else {
        return status(CollectionStatus::Unavailable);
    };
    // The config owner already enforces ownership, lexical and symlink boundaries.
    // External/user-owned catalogs are deliberately not opened by diagnostics.
    let Some(path) = crate::codex_config::resolve_cc_switch_catalog_path(config_text, base) else {
        return status(CollectionStatus::NotSupported);
    };
    let text = match read_bounded(&path, MAX_CATALOG_BYTES) {
        Ok(text) => text,
        Err(error) => return status(error),
    };
    let catalog: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return status(CollectionStatus::Unavailable),
    };
    let Some(models) = catalog.get("models").and_then(Value::as_array) else {
        return status(CollectionStatus::Unavailable);
    };
    let Some(model) = model else {
        return status(CollectionStatus::NotConfigured);
    };
    let mut matched = models
        .iter()
        .filter(|entry| entry.get("slug").and_then(Value::as_str) == Some(model));
    let Some(entry) = matched.next() else {
        return status(CollectionStatus::Unavailable);
    };
    if matched.next().is_some() {
        return status(CollectionStatus::Unavailable);
    }
    let levels_state = match entry.get("supported_reasoning_levels") {
        None => "absent",
        Some(Value::Array(levels)) if levels.is_empty() => "explicit_empty",
        Some(Value::Array(_)) => "present",
        Some(_) => "invalid",
    };
    collected(json!({
        "modelAlias":"model-1",
        "publicModelId":public_model_id(model),
        "contextWindow":catalog_number(entry, "context_window"),
        "maxContextWindow":catalog_number(entry, "max_context_window"),
        "autoCompactTokenLimit":catalog_number(entry, "auto_compact_token_limit"),
        "reasoningLevelsState":levels_state,
        "reasoning": reasoning_projection(crate::codex_reasoning::native_capabilities(entry)),
    }))
}

/// Observes top-level disk configuration only. App intent and a running Codex
/// process are separate facts; this read cannot establish either one.
pub(crate) fn codex_snapshot(config_path: &Path) -> Value {
    let mut snapshot = json!({
        "configurationIntent": status(CollectionStatus::NotObserved),
        "runtimeObservation": status(CollectionStatus::NotObserved),
        "diskConfig": status(CollectionStatus::NotObserved),
        "diskCatalog": status(CollectionStatus::NotObserved),
    });
    let text = match read_bounded(config_path, MAX_CONFIG_BYTES) {
        Ok(text) => text,
        Err(error) => {
            snapshot["diskConfig"] = status(error);
            return snapshot;
        }
    };
    let doc = match text.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => {
            snapshot["diskConfig"] = status(CollectionStatus::Unavailable);
            return snapshot;
        }
    };
    // CLI-selected profiles can override top-level fields. Do not present those
    // fields as the selected model when this collector cannot resolve that scope.
    if doc.get("profile").is_some() {
        snapshot["diskConfig"] = status(CollectionStatus::NotSupported);
        return snapshot;
    }
    let model = doc
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty());
    let effort = doc.get("model_reasoning_effort");
    let provider = crate::codex_config::active_codex_model_provider_id(&doc);
    let wire = provider
        .as_deref()
        .and_then(|id| doc.get("model_providers")?.get(id)?.get("wire_api"));
    snapshot["diskConfig"] = collected(json!({
        "scope":"top_level",
        "modelAlias":model.map(|_| "model-1"),
        "publicModelId":model.and_then(public_model_id),
        "modelStatus": if model.is_some() {CollectionStatus::Ok} else if doc.get("model").is_some() {CollectionStatus::Unavailable} else {CollectionStatus::NotConfigured},
        "reasoningEffort":enum_field(effort.and_then(|v| v.as_str()), crate::codex_reasoning::EFFORTS, effort.is_some()),
        "wireApi":enum_field(wire.and_then(|v| v.as_str()), &["responses","chat"], wire.is_some()),
        "contextWindow":config_number(&doc, "model_context_window"),
        "autoCompactTokenLimit":config_number(&doc, "model_auto_compact_token_limit"),
    }));
    snapshot["diskCatalog"] = selected_catalog_snapshot(&text, config_path, model, &doc);
    snapshot
}

/// Collect only explicitly selected applications. The caller owns the diagnostics
/// consent gate; an empty selection never resolves a path or opens a file.
pub(crate) fn selected_application_snapshots(selected: &[String]) -> Value {
    selected_application_snapshots_with(selected, |app| match app {
        "codex" => codex_snapshot(&crate::codex_config::get_codex_config_path()),
        "claude" => claude_snapshot(&crate::config::get_claude_settings_path()),
        "pi" => match (
            crate::pi_config::get_pi_settings_path(),
            crate::pi_config::get_pi_models_path(),
        ) {
            (Ok(settings), Ok(models)) => pi_snapshot(&settings, &models),
            _ => status(CollectionStatus::Unavailable),
        },
        _ => status(CollectionStatus::NotSupported),
    })
}

fn selected_application_snapshots_with(
    selected: &[String],
    mut collect: impl FnMut(&str) -> Value,
) -> Value {
    let mut result = serde_json::Map::new();
    for app in selected {
        let key = match app.as_str() {
            "codex" | "claude" | "pi" | "gemini" | "grokbuild" | "opencode" | "openclaw"
            | "hermes" | "claude-desktop" | "codex-image" => app.as_str(),
            _ => "unknown",
        };
        if result.contains_key(key) {
            continue;
        }
        let snapshot = match key {
            "codex" | "claude" | "pi" => collect(key),
            _ => status(CollectionStatus::NotSupported),
        };
        result.insert(key.to_owned(), snapshot);
    }
    Value::Object(result)
}

fn disk_snapshot() -> Value {
    json!({
        "configurationIntent":status(CollectionStatus::NotObserved),
        "runtimeObservation":status(CollectionStatus::NotObserved),
        "diskConfig":status(CollectionStatus::NotObserved),
    })
}

fn read_safe_json(path: &Path, max_bytes: u64, json5: bool) -> Result<Value, CollectionStatus> {
    let text = read_bounded(path, max_bytes)?;
    // Pi's native owner uses JSON5. Claude's settings owner uses plain JSON.
    let value: Value = if json5 {
        json5::from_str(&text).map_err(|_| CollectionStatus::Unavailable)?
    } else {
        serde_json::from_str(&text).map_err(|_| CollectionStatus::Unavailable)?
    };
    if value.is_object() {
        Ok(value)
    } else {
        Err(CollectionStatus::Unavailable)
    }
}

fn model_field(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(model)) if !model.trim().is_empty() => json!({
            "status":CollectionStatus::Ok,
            "modelAlias":"model-1",
            "publicModelId":public_model_id(model),
        }),
        Some(_) => status(CollectionStatus::Unavailable),
        None => status(CollectionStatus::NotConfigured),
    }
}

fn boolean_field(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Bool(value)) => json!({"status":CollectionStatus::Ok,"value":value}),
        Some(_) => status(CollectionStatus::Unavailable),
        None => status(CollectionStatus::NotConfigured),
    }
}

fn claude_snapshot(path: &Path) -> Value {
    let mut snapshot = disk_snapshot();
    let config = match read_safe_json(path, MAX_CONFIG_BYTES, false) {
        Ok(config) => config,
        Err(error) => {
            snapshot["diskConfig"] = status(error);
            return snapshot;
        }
    };
    let env = config.get("env");
    if env.is_some_and(|env| !env.is_object()) {
        snapshot["diskConfig"] = status(CollectionStatus::Unavailable);
        return snapshot;
    }
    let model = env.and_then(|env| env.get("ANTHROPIC_MODEL"));
    let effort = env.and_then(|env| env.get("CLAUDE_CODE_EFFORT_LEVEL"));
    let top_effort = config.get("effortLevel");
    // These are two independent declarations, not precedence or effective state.
    // All unrecognised values are omitted; arbitrary env keys never leave here.
    // Official Claude Code model-config docs (read 2026-10-18) define different
    // enums for the environment variable and persistent settings.effortLevel.
    let environment_efforts = ["low", "medium", "high", "xhigh", "max", "auto"];
    let settings_efforts = ["low", "medium", "high", "xhigh"];
    let settings_model = config.get("model");
    let mut settings_model_projection = model_field(settings_model);
    if model
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        && model != settings_model
        && settings_model_projection.get("modelAlias").is_some()
    {
        settings_model_projection["modelAlias"] = json!("model-2");
    }
    snapshot["diskConfig"] = collected(json!({
        "scope":"user_settings",
        "environmentModel":model_field(model),
        "settingsModel":settings_model_projection,
        "reasoningEffort":enum_field(effort.and_then(Value::as_str), &environment_efforts, effort.is_some()),
        "effortLevel":enum_field(top_effort.and_then(Value::as_str), &settings_efforts, top_effort.is_some()),
        "modelSettings":{"status":CollectionStatus::NotSupported,"present":config.get("modelSettings").is_some()},
        "protocol":status(CollectionStatus::NotSupported),
        "contextWindow":status(CollectionStatus::NotSupported),
    }));
    snapshot
}

// Native Pi 1.3.0, commit 482c1381a00d8d00bc81697e4a980c593db6803e,
// packages/coding-agent/docs/settings.md: disk defaultThinkingLevel vocabulary.
const PI_THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

fn pi_thinking_map(value: Option<&Value>) -> Value {
    // Declaration keys and null semantics come from piThinkingProfiles.ts and
    // PiProviderForm.tsx. This is not capability inference or profile resolution.
    // Output vocabulary is distinct from input keys. Keep original spelling
    // and case from reviewed native piThinkingProfiles.ts; a test checks drift.
    const WIRE_VALUES: &[&str] = &[
        "minimal", "low", "medium", "high", "xhigh", "max", "none", "LOW", "HIGH",
    ];
    let Some(value) = value else {
        return status(CollectionStatus::NotConfigured);
    };
    let Some(map) = value.as_object() else {
        return status(CollectionStatus::Unavailable);
    };
    if map
        .keys()
        .any(|key| !PI_THINKING_LEVELS.contains(&key.as_str()))
    {
        return status(CollectionStatus::Unavailable);
    }
    let mut projected = serde_json::Map::new();
    for level in PI_THINKING_LEVELS {
        let field = match map.get(*level) {
            None => status(CollectionStatus::NotConfigured),
            Some(Value::Null) => status(CollectionStatus::NotSupported),
            Some(value) => enum_field(value.as_str(), WIRE_VALUES, true),
        };
        projected.insert((*level).to_owned(), field);
    }
    let mut result = collected(Value::Object(projected));
    result["declaration"] = json!(if map.is_empty() {
        "explicit_empty"
    } else {
        "present"
    });
    result
}

fn pi_snapshot(settings_path: &Path, models_path: &Path) -> Value {
    let mut snapshot = disk_snapshot();
    snapshot["diskModels"] = status(CollectionStatus::NotObserved);
    let settings = match read_safe_json(settings_path, MAX_CONFIG_BYTES, true) {
        Ok(settings) => settings,
        Err(error) => {
            snapshot["diskConfig"] = status(error);
            return snapshot;
        }
    };
    let provider = settings.get("defaultProvider");
    let model = settings.get("defaultModel");
    let thinking = settings.get("defaultThinkingLevel");
    snapshot["diskConfig"] = collected(json!({
        "scope":"user_settings",
        "model":model_field(model),
        "defaultThinkingLevel":enum_field(thinking.and_then(Value::as_str), PI_THINKING_LEVELS, thinking.is_some()),
        "modelThinkingLevels":{"status":CollectionStatus::NotSupported,"present":settings.get("modelThinkingLevels").is_some()},
        "providerStatus":match provider {
            Some(Value::String(value)) if !value.trim().is_empty() => CollectionStatus::Ok,
            Some(_) => CollectionStatus::Unavailable,
            None => CollectionStatus::NotConfigured,
        },
    }));
    let selection = provider
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .zip(
            model
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty()),
        );
    let Some((provider, model)) = selection else {
        snapshot["diskModels"] = status(if provider.is_none() || model.is_none() {
            CollectionStatus::NotConfigured
        } else {
            CollectionStatus::Unavailable
        });
        return snapshot;
    };
    // Match the native Pi owner limit. Never open auth.json, sessions or prompts.
    let models = match read_safe_json(models_path, 1024 * 1024, true) {
        Ok(models) => models,
        Err(error) => {
            snapshot["diskModels"] = status(error);
            return snapshot;
        }
    };
    let providers = match models.get("providers") {
        Some(Value::Object(providers)) => providers,
        value => {
            snapshot["diskModels"] = status(if value.is_none() {
                CollectionStatus::NotConfigured
            } else {
                CollectionStatus::Unavailable
            });
            return snapshot;
        }
    };
    let Some(provider) = providers.get(provider) else {
        snapshot["diskModels"] = status(CollectionStatus::NotConfigured);
        return snapshot;
    };
    let Some(rows) = provider.get("models").and_then(Value::as_array) else {
        snapshot["diskModels"] = status(CollectionStatus::Unavailable);
        return snapshot;
    };
    let mut matches = rows
        .iter()
        .filter(|row| row.get("id").and_then(Value::as_str) == Some(model));
    let Some(row) = matches.next() else {
        snapshot["diskModels"] = status(CollectionStatus::NotConfigured);
        return snapshot;
    };
    if matches.next().is_some() {
        snapshot["diskModels"] = status(CollectionStatus::Unavailable);
        return snapshot;
    }
    // API enum is the native Pi form's exact public allowlist. No baseUrl,
    // headers, compatibility object or arbitrary model parameters are copied.
    let provider_api = provider.get("api");
    let model_api = row.get("api");
    let protocols = [
        "openai-completions",
        "openai-responses",
        "anthropic-messages",
        "google-generative-ai",
        "bedrock-converse-stream",
    ];
    snapshot["diskModels"] = collected(json!({
        "model":model_field(row.get("id")),
        "providerProtocol":enum_field(provider_api.and_then(Value::as_str), &protocols, provider_api.is_some()),
        "modelProtocol":enum_field(model_api.and_then(Value::as_str), &protocols, model_api.is_some()),
        "reasoning":boolean_field(row.get("reasoning")),
        "contextWindow":catalog_number(row, "contextWindow"),
        "maxTokens":catalog_number(row, "maxTokens"),
        "thinkingLevelMap":pi_thinking_map(row.get("thinkingLevelMap")),
    }));
    snapshot
}

/// Input errors are category codes supplied by the local probe owner, never
/// stderr. Version suffixes/build metadata can be private; export booleans only.
pub(crate) fn tool_version_snapshot(result: Result<&str, &str>) -> Value {
    match result {
        Ok(raw) => match semver::Version::parse(raw) {
            Ok(version) => {
                let pre = version.pre.as_str();
                let public_pre = ["alpha", "beta", "rc"].iter().any(|channel| {
                    pre.strip_prefix(channel).is_some_and(|suffix| {
                        let number = suffix.strip_prefix('.').unwrap_or(suffix);
                        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                    })
                });
                let mut safe_version =
                    format!("{}.{}.{}", version.major, version.minor, version.patch);
                if public_pre {
                    safe_version.push('-');
                    safe_version.push_str(pre);
                }
                collected(json!({
                    "version":safe_version,
                    "versionExact":version.build.is_empty() && (pre.is_empty() || public_pre),
                    "prerelease":!pre.is_empty(),
                    "buildMetadata":!version.build.is_empty(),
                }))
            }
            Err(_) => status(CollectionStatus::Unavailable),
        },
        Err("not_configured") => status(CollectionStatus::NotConfigured),
        Err("not_supported") => status(CollectionStatus::NotSupported),
        Err("timeout") => status(CollectionStatus::Timeout),
        Err("truncated") => status(CollectionStatus::Truncated),
        Err(_) => status(CollectionStatus::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    const CANARY: &str = "PRIVATE_CANARY_8675309";

    fn fixture(config: &str, catalog: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, config).unwrap();
        fs::write(dir.path().join("loongport-model-catalog.json"), catalog).unwrap();
        (dir, path)
    }
    fn selected_config() -> String {
        format!("model = '{CANARY}'\nmodel_catalog_json = 'loongport-model-catalog.json'\nmodel_reasoning_effort = 'high'\nmodel_context_window = 1000000\nmodel_auto_compact_token_limit = 900000\nmodel_provider = '{CANARY}'\nbase_url = 'https://{CANARY}.example'\napi_key = '{CANARY}'\ninstructions = '{CANARY}'\n[model_providers.{CANARY}]\nwire_api = 'responses'\nbase_url = 'https://{CANARY}.example'\nexperimental_bearer_token = '{CANARY}'\n")
    }
    fn selected_catalog() -> String {
        json!({"models":[{"slug":CANARY,"display_name":CANARY,"base_instructions":CANARY,"context_window":272000,"max_context_window":1000000,"auto_compact_token_limit":244800,"supported_reasoning_levels":[{"effort":"low","description":CANARY},{"effort":"high"},{"effort":CANARY}],"default_reasoning_level":"high","custom_parameter":CANARY},{"slug":"other-private-model","base_instructions":CANARY}]}).to_string()
    }
    #[test]
    fn no_consent_has_no_detailed_collection_or_manifest_fields() {
        let manifest = environment_manifest(false, || panic!("must not collect details"));
        assert_eq!(
            {
                let mut keys = manifest
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                keys.sort();
                keys
            },
            vec!["appVersion", "arch", "family", "os"]
        );
        assert!(manifest.get("diagnostics").is_none());
    }
    #[test]
    fn consent_collects_details_once() {
        let calls = std::cell::Cell::new(0);
        let manifest = environment_manifest(true, || {
            calls.set(calls.get() + 1);
            json!({"status":"ok"})
        });
        assert_eq!(calls.get(), 1);
        assert_eq!(manifest["diagnostics"]["status"], "ok");
    }
    #[test]
    fn selected_model_projection_is_allowlisted_and_never_asserts_runtime() {
        let (_dir, path) = fixture(&selected_config(), &selected_catalog());
        let result = codex_snapshot(&path);
        let serialized = result.to_string();
        for secret in [
            CANARY,
            "other-private-model",
            "https://",
            "config.toml",
            "loongport-model-catalog.json",
        ] {
            assert!(
                !serialized.contains(secret),
                "unexpected private content: {secret}"
            );
        }
        assert_eq!(result["diskConfig"]["status"], "ok");
        assert_eq!(result["diskConfig"]["data"]["modelAlias"], "model-1");
        assert_eq!(
            result["diskConfig"]["data"]["reasoningEffort"]["value"],
            "high"
        );
        assert_eq!(
            result["diskCatalog"]["data"]["contextWindow"]["value"],
            272000
        );
        assert_eq!(
            result["diskCatalog"]["data"]["reasoning"]["levels"],
            json!(["low", "high"])
        );
        assert_eq!(result["runtimeObservation"]["status"], "not_observed");
        assert_eq!(result["configurationIntent"]["status"], "not_observed");
    }
    #[test]
    fn malformed_and_oversized_inputs_have_explicit_failure_states() {
        let (_dir, path) = fixture("model = [", "{}");
        assert_eq!(codex_snapshot(&path)["diskConfig"]["status"], "unavailable");
        fs::write(&path, "x".repeat(MAX_CONFIG_BYTES as usize + 1)).unwrap();
        assert_eq!(codex_snapshot(&path)["diskConfig"]["status"], "truncated");
        fs::write(&path, selected_config()).unwrap();
        fs::write(
            path.parent().unwrap().join("loongport-model-catalog.json"),
            "{",
        )
        .unwrap();
        assert_eq!(
            codex_snapshot(&path)["diskCatalog"]["status"],
            "unavailable"
        );
        fs::write(
            path.parent().unwrap().join("loongport-model-catalog.json"),
            "x".repeat(MAX_CATALOG_BYTES as usize + 1),
        )
        .unwrap();
        assert_eq!(codex_snapshot(&path)["diskCatalog"]["status"], "truncated");
    }
    #[test]
    fn missing_files_and_external_catalog_are_not_silent_successes() {
        let (dir, path) = fixture("", "{}");
        fs::remove_file(&path).unwrap();
        assert_eq!(
            codex_snapshot(&path)["diskConfig"]["status"],
            "not_configured"
        );
        fs::write(
            &path,
            "model='private'\nmodel_catalog_json='private-catalog.json'",
        )
        .unwrap();
        assert_eq!(
            codex_snapshot(&path)["diskCatalog"]["status"],
            "not_supported"
        );
        fs::write(&path, selected_config()).unwrap();
        fs::remove_file(dir.path().join("loongport-model-catalog.json")).unwrap();
        assert_eq!(
            codex_snapshot(&path)["diskCatalog"]["status"],
            "not_configured"
        );
    }
    #[test]
    fn absent_and_explicit_empty_reasoning_stay_distinct_without_inventing_unsupported() {
        for (entry, state) in [
            (json!({"slug":CANARY}), "absent"),
            (
                json!({"slug":CANARY,"supported_reasoning_levels":[]}),
                "explicit_empty",
            ),
        ] {
            let (_dir, path) = fixture(&selected_config(), &json!({"models":[entry]}).to_string());
            let snapshot = codex_snapshot(&path);
            assert_eq!(
                snapshot["diskCatalog"]["data"]["reasoningLevelsState"],
                state
            );
            assert_eq!(
                snapshot["diskCatalog"]["data"]["reasoning"]["mode"],
                "unknown"
            );
        }
    }
    #[test]
    fn tool_projection_discards_errors_and_prerelease_values() {
        let tool = tool_version_snapshot(Ok("1.2.3-PRIVATE_CANARY_8675309"));
        assert_eq!(tool["status"], "unavailable");
        let tool = tool_version_snapshot(Ok("1.2.3-private.secret+private.metadata"));
        assert_eq!(tool["data"]["version"], "1.2.3");
        assert_eq!(tool["data"]["prerelease"], true);
        assert!(!tool.to_string().contains("private"));
        let failure = tool_version_snapshot(Err(CANARY));
        assert_eq!(failure["status"], "unavailable");
        assert!(!failure.to_string().contains(CANARY));
        assert_eq!(tool_version_snapshot(Err("timeout"))["status"], "timeout");
        assert_eq!(
            tool_version_snapshot(Err("not_configured"))["status"],
            "not_configured"
        );
        assert_eq!(
            tool_version_snapshot(Err("not_supported"))["status"],
            "not_supported"
        );
    }
    #[cfg(unix)]
    #[test]
    fn permission_denied_is_unavailable_and_contains_no_path() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = fixture(&selected_config(), &selected_catalog());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let result = codex_snapshot(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(result["diskConfig"]["status"], "unavailable");
        assert!(!result.to_string().contains(path.to_str().unwrap()));
    }
    #[test]
    fn capability_unknown_and_unsupported_are_not_conflated() {
        for mode in ["unknown", "unsupported"] {
            let capability = crate::codex_reasoning::declared_capabilities(
                &json!({"reasoningMode":mode,"reasoningLevels":[]}),
            );
            assert_eq!(reasoning_projection(capability)["mode"], mode);
        }
    }

    #[test]
    fn empty_config_profile_selection_and_duplicate_rows_are_explicit() {
        let (_dir, path) = fixture("", "{}");
        let result = codex_snapshot(&path);
        assert_eq!(
            result["diskConfig"]["data"]["modelStatus"],
            "not_configured"
        );
        assert_eq!(result["diskCatalog"]["status"], "not_configured");
        fs::write(&path, "profile='PRIVATE_CANARY_8675309'").unwrap();
        let result = codex_snapshot(&path);
        assert_eq!(result["diskConfig"]["status"], "not_supported");
        assert_eq!(result["diskCatalog"]["status"], "not_observed");
        assert!(!result.to_string().contains(CANARY));
        fs::write(&path, selected_config()).unwrap();
        fs::write(
            path.parent().unwrap().join("loongport-model-catalog.json"),
            json!({"models":[{"slug":CANARY},{"slug":CANARY}]}).to_string(),
        )
        .unwrap();
        assert_eq!(
            codex_snapshot(&path)["diskCatalog"]["status"],
            "unavailable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn catalog_symlink_escape_is_not_opened() {
        let (dir, path) = fixture(&selected_config(), "{}");
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("loongport-model-catalog.json");
        fs::write(&target, selected_catalog()).unwrap();
        let catalog = dir.path().join("loongport-model-catalog.json");
        fs::remove_file(&catalog).unwrap();
        std::os::unix::fs::symlink(&target, &catalog).unwrap();
        let result = codex_snapshot(&path);
        assert_eq!(result["diskCatalog"]["status"], "not_supported");
        assert!(!result
            .to_string()
            .contains(outside.path().to_str().unwrap()));
    }
    #[cfg(unix)]
    #[test]
    fn config_symlink_is_never_read() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("private-target");
        fs::write(&target, selected_config()).unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            codex_snapshot(&link)["diskConfig"]["status"],
            "not_supported"
        );
    }

    #[test]
    fn official_seed_ids_remain_recognizable_without_exposing_private_aliases() {
        let official = crate::codex_reasoning::seed_official_model_facts();
        let model = official[0]["slug"].as_str().unwrap();
        let config =
            format!("model='{model}'\nmodel_catalog_json='loongport-model-catalog.json'\n");
        let (_dir, path) = fixture(&config, &json!({"models":[{"slug":model}]}).to_string());
        let result = codex_snapshot(&path);
        assert_eq!(result["diskConfig"]["data"]["publicModelId"], model);
        assert_eq!(result["diskCatalog"]["data"]["publicModelId"], model);
        let (_dir, path) = fixture(&selected_config(), &selected_catalog());
        let result = codex_snapshot(&path);
        assert!(result["diskConfig"]["data"]["publicModelId"].is_null());
        assert!(!result.to_string().contains(CANARY));
    }

    #[test]
    fn prerelease_versions_preserve_only_public_channels_and_declare_exactness() {
        for channel in ["alpha", "beta", "rc"] {
            let version = format!("1.2.3-{channel}.4");
            let snapshot = tool_version_snapshot(Ok(&version));
            assert_eq!(snapshot["data"]["version"], version);
            assert_eq!(snapshot["data"]["versionExact"], true);
        }
        for raw in [
            "1.2.3-private.secret",
            "1.2.3-beta.4+private",
            "1.2.3-beta.4.secret",
        ] {
            let snapshot = tool_version_snapshot(Ok(raw));
            assert_eq!(snapshot["data"]["versionExact"], false);
            assert!(!snapshot.to_string().contains("private"));
            assert!(!snapshot.to_string().contains("secret"));
        }
        assert_eq!(
            tool_version_snapshot(Ok("1.2.3"))["data"]["versionExact"],
            true
        );
    }
    #[cfg(unix)]
    #[test]
    fn opened_handle_never_reopens_a_replaced_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "model='safe'").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        fs::rename(&path, dir.path().join("old-config")).unwrap();
        let private = dir.path().join("private-target");
        fs::write(&private, CANARY).unwrap();
        std::os::unix::fs::symlink(&private, &path).unwrap();
        assert_eq!(
            read_open_file_bounded(file, MAX_CONFIG_BYTES).unwrap(),
            "model='safe'"
        );
        assert!(matches!(
            read_bounded(&path, MAX_CONFIG_BYTES),
            Err(CollectionStatus::NotSupported)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_files_are_rejected_without_blocking_fifo_open() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_bounded(dir.path(), MAX_CONFIG_BYTES),
            Err(CollectionStatus::NotSupported)
        ));
        let fifo = dir.path().join("synthetic-fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: the C string is live and this creates only a synthetic temp fixture.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(matches!(
            read_bounded(&fifo, MAX_CONFIG_BYTES),
            Err(CollectionStatus::NotSupported)
        ));
    }
}

#[cfg(test)]
mod selected_application_tests {
    use super::*;
    use std::fs;
    const PRIVATE: &str = "PRIVATE_APPLICATION_CANARY";

    #[test]
    fn empty_selection_never_resolves_or_reads_application_paths() {
        assert_eq!(selected_application_snapshots(&[]), json!({}));
        assert_eq!(
            selected_application_snapshots_with(&[], |_| panic!("no collection")),
            json!({})
        );
    }

    #[test]
    fn selected_applications_are_deduplicated_and_unknown_input_never_echoes() {
        let mut calls = Vec::new();
        let result = selected_application_snapshots_with(
            &[
                "claude".into(),
                "claude".into(),
                "pi".into(),
                "gemini".into(),
                PRIVATE.into(),
            ],
            |app| {
                calls.push(app.to_owned());
                json!({"selected":true})
            },
        );
        assert_eq!(calls, vec!["claude", "pi"]);
        assert_eq!(result["gemini"]["status"], "not_supported");
        assert_eq!(result["unknown"]["status"], "not_supported");
        assert!(!result.to_string().contains(PRIVATE));
        assert!(result.get("codex").is_none());
    }

    #[test]
    fn claude_only_exports_owned_model_and_effort_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, json!({"env":{"ANTHROPIC_MODEL":PRIVATE,"CLAUDE_CODE_EFFORT_LEVEL":"max","ANTHROPIC_AUTH_TOKEN":PRIVATE,"ANTHROPIC_BASE_URL":format!("https://{PRIVATE}.example"),"MCP_ENV":PRIVATE},"model":PRIVATE,"systemPrompt":PRIVATE,"mcpServers":{"private":PRIVATE},"headers":{"Authorization":PRIVATE},"custom":PRIVATE}).to_string()).unwrap();
        let value = claude_snapshot(&path);
        assert_eq!(
            value["diskConfig"]["data"]["environmentModel"]["modelAlias"],
            "model-1"
        );
        assert_eq!(
            value["diskConfig"]["data"]["reasoningEffort"]["value"],
            "max"
        );
        assert_eq!(
            value["diskConfig"]["data"]["protocol"]["status"],
            "not_supported"
        );
        assert_eq!(value["runtimeObservation"]["status"], "not_observed");
        assert!(!value.to_string().contains(PRIVATE));
    }

    #[test]
    fn claude_disk_effort_sources_are_separate_not_an_effective_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            r#"{"effortLevel":"high","env":{"CLAUDE_CODE_EFFORT_LEVEL":"max"}}"#,
        )
        .unwrap();
        let value = claude_snapshot(&path);
        assert_eq!(value["diskConfig"]["data"]["effortLevel"]["value"], "high");
        assert_eq!(
            value["diskConfig"]["data"]["reasoningEffort"]["value"],
            "max"
        );
        assert_eq!(value["configurationIntent"]["status"], "not_observed");
    }

    #[test]
    fn claude_model_declarations_are_separate_and_aliases_do_not_conflate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, json!({"model":PRIVATE}).to_string()).unwrap();
        let value = claude_snapshot(&path);
        assert_eq!(
            value["diskConfig"]["data"]["settingsModel"]["modelAlias"],
            "model-1"
        );
        assert_eq!(
            value["diskConfig"]["data"]["environmentModel"]["status"],
            "not_configured"
        );
        fs::write(&path,json!({"model":"other-private-model","env":{"ANTHROPIC_MODEL":PRIVATE},"modelSettings":{PRIVATE:{"effort":"xhigh","private":PRIVATE}}}).to_string()).unwrap();
        let value = claude_snapshot(&path);
        let data = &value["diskConfig"]["data"];
        assert_eq!(data["environmentModel"]["modelAlias"], "model-1");
        assert_eq!(data["settingsModel"]["modelAlias"], "model-2");
        assert_eq!(data["modelSettings"]["status"], "not_supported");
        assert_eq!(data["modelSettings"]["present"], true);
        assert!(!value.to_string().contains(PRIVATE));
        assert!(!value.to_string().contains("other-private-model"));
    }

    #[test]
    fn pi_known_preset_wire_values_preserve_case_and_do_not_drift() {
        let source = include_str!("../../src/config/piThinkingProfiles.ts");
        let profiles = source
            .split("export const piThinkingProfiles =")
            .nth(1)
            .unwrap()
            .split("} as const satisfies Record<string, PiThinkingProfile>")
            .next()
            .unwrap();
        let re = regex::Regex::new(r#":\s*"([^"]+)""#).unwrap();
        let mut observed = std::collections::BTreeSet::new();
        for captures in re.captures_iter(profiles) {
            let wire = &captures[1];
            observed.insert(wire.to_owned());
            let projected = pi_thinking_map(Some(&json!({"high":wire})));
            assert_eq!(
                projected["data"]["high"]["value"], wire,
                "preset wire value {wire}"
            );
        }
        for expected in ["none", "LOW", "HIGH"] {
            assert!(observed.contains(expected));
        }
        assert_eq!(
            pi_thinking_map(Some(&json!({"high":PRIVATE})))["data"]["high"]["status"],
            "unavailable"
        );
    }

    #[test]
    fn pi_provider_and_model_protocol_are_separate_disk_declarations() {
        let (_dir, settings, models) = pi_files();
        fs::write(&settings, r#"{"defaultProvider":"p","defaultModel":"m"}"#).unwrap();
        for provider_api in [Some("openai-completions"), None] {
            let mut provider = json!({"models":[{"id":"m","api":"anthropic-messages"}]});
            if let Some(api) = provider_api {
                provider["api"] = json!(api);
            }
            fs::write(&models, json!({"providers":{"p":provider}}).to_string()).unwrap();
            let value = pi_snapshot(&settings, &models);
            let data = &value["diskModels"]["data"];
            assert_eq!(data["modelProtocol"]["value"], "anthropic-messages");
            assert_eq!(
                data["providerProtocol"]["status"],
                if provider_api.is_some() {
                    "ok"
                } else {
                    "not_configured"
                }
            );
            assert!(data.get("protocol").is_none());
            assert_eq!(value["runtimeObservation"]["status"], "not_observed");
        }
    }

    #[test]
    fn claude_environment_effort_preserves_xhigh_and_auto_declarations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for effort in ["xhigh", "auto"] {
            fs::write(
                &path,
                json!({"env":{"CLAUDE_CODE_EFFORT_LEVEL":effort}}).to_string(),
            )
            .unwrap();
            let value = claude_snapshot(&path);
            assert_eq!(
                value["diskConfig"]["data"]["reasoningEffort"]["value"],
                effort
            );
            assert_eq!(value["runtimeObservation"]["status"], "not_observed");
        }
    }

    #[test]
    fn claude_settings_effort_has_its_own_enum_and_rejects_max() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"effortLevel":"xhigh"}"#).unwrap();
        assert_eq!(
            claude_snapshot(&path)["diskConfig"]["data"]["effortLevel"]["value"],
            "xhigh"
        );
        for invalid in [
            json!("max"),
            json!("auto"),
            json!(PRIVATE),
            json!(null),
            json!(42),
        ] {
            fs::write(&path, json!({"effortLevel":invalid}).to_string()).unwrap();
            assert_eq!(
                claude_snapshot(&path)["diskConfig"]["data"]["effortLevel"]["status"],
                "unavailable"
            );
        }
    }

    #[test]
    fn claude_missing_invalid_and_empty_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(
            claude_snapshot(&path)["diskConfig"]["status"],
            "not_configured"
        );
        for invalid in ["[", "[]", "null"] {
            fs::write(&path, invalid).unwrap();
            assert_eq!(
                claude_snapshot(&path)["diskConfig"]["status"],
                "unavailable"
            );
        }
        fs::write(&path, "{}").unwrap();
        let value = claude_snapshot(&path);
        assert_eq!(
            value["diskConfig"]["data"]["environmentModel"]["status"],
            "not_configured"
        );
        fs::write(
            &path,
            r#"{"env":{"ANTHROPIC_MODEL":"","CLAUDE_CODE_EFFORT_LEVEL":"private-effort"}}"#,
        )
        .unwrap();
        let value = claude_snapshot(&path);
        assert_eq!(
            value["diskConfig"]["data"]["environmentModel"]["status"],
            "unavailable"
        );
        assert_eq!(
            value["diskConfig"]["data"]["reasoningEffort"]["status"],
            "unavailable"
        );
        assert!(!value.to_string().contains("private-effort"));
    }

    fn pi_files() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        let models = dir.path().join("models.json");
        fs::write(&settings,format!("{{// native JSON5\n defaultProvider:'{PRIVATE}',defaultModel:'{PRIVATE}',sessionDir:'/{PRIVATE}',defaultThinkingLevel:'{PRIVATE}',}} ")).unwrap();
        fs::write(&models,json!({"providers":{PRIVATE:{"api":"openai-responses","baseUrl":PRIVATE,"apiKey":PRIVATE,"headers":{"Authorization":PRIVATE},"models":[{"id":PRIVATE,"name":PRIVATE,"reasoning":true,"contextWindow":1000000,"maxTokens":8192,"thinkingLevelMap":{"low":null,"high":"high","max":PRIVATE},"prompt":PRIVATE},{"id":"other-private","contextWindow":42}]},"other-private-provider":{"apiKey":PRIVATE}}}).to_string()).unwrap();
        (dir, settings, models)
    }

    #[test]
    fn pi_selected_model_is_projected_from_json5_without_other_rows_or_credentials() {
        let (_dir, settings, models) = pi_files();
        let value = pi_snapshot(&settings, &models);
        let data = &value["diskModels"]["data"];
        assert_eq!(
            value["diskConfig"]["data"]["model"]["modelAlias"],
            "model-1"
        );
        assert_eq!(data["providerProtocol"]["value"], "openai-responses");
        assert_eq!(data["reasoning"]["value"], true);
        assert_eq!(data["contextWindow"]["value"], 1000000);
        assert_eq!(data["maxTokens"]["value"], 8192);
        assert_eq!(
            data["thinkingLevelMap"]["data"]["low"]["status"],
            "not_supported"
        );
        assert_eq!(data["thinkingLevelMap"]["data"]["high"]["value"], "high");
        assert_eq!(
            data["thinkingLevelMap"]["data"]["max"]["status"],
            "unavailable"
        );
        assert_eq!(value["runtimeObservation"]["status"], "not_observed");
        for private in [PRIVATE, "other-private", "sessionDir", "apiKey"] {
            assert!(!value.to_string().contains(private), "{private}");
        }
    }

    #[test]
    fn pi_default_thinking_level_is_a_safe_disk_declaration() {
        let (_dir, settings, models) = pi_files();
        for effort in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
            fs::write(
                &settings,
                json!({"defaultThinkingLevel":effort}).to_string(),
            )
            .unwrap();
            let value = pi_snapshot(&settings, &models);
            assert_eq!(
                value["diskConfig"]["data"]["defaultThinkingLevel"]["value"],
                effort
            );
            assert_eq!(value["diskModels"]["status"], "not_configured");
            assert_eq!(value["runtimeObservation"]["status"], "not_observed");
        }
        for invalid in [
            json!(PRIVATE),
            json!(null),
            json!(false),
            json!({"private":PRIVATE}),
        ] {
            fs::write(
                &settings,
                json!({"defaultThinkingLevel":invalid}).to_string(),
            )
            .unwrap();
            let value = pi_snapshot(&settings, &models);
            assert_eq!(
                value["diskConfig"]["data"]["defaultThinkingLevel"]["status"],
                "unavailable"
            );
            assert!(!value.to_string().contains(PRIVATE));
        }
        fs::write(&settings, "{}").unwrap();
        assert_eq!(
            pi_snapshot(&settings, &models)["diskConfig"]["data"]["defaultThinkingLevel"]["status"],
            "not_configured"
        );
    }

    #[test]
    fn pi_missing_selection_does_not_read_models_and_duplicate_rows_are_unavailable() {
        let (_dir, settings, models) = pi_files();
        fs::write(&settings, "{}").unwrap();
        fs::write(&models, "bad JSON").unwrap();
        assert_eq!(
            pi_snapshot(&settings, &models)["diskModels"]["status"],
            "not_configured"
        );
        fs::write(&settings, r#"{"defaultProvider":"p","defaultModel":"m"}"#).unwrap();
        fs::write(
            &models,
            r#"{"providers":{"p":{"models":[{"id":"m"},{"id":"m"}]}}}"#,
        )
        .unwrap();
        assert_eq!(
            pi_snapshot(&settings, &models)["diskModels"]["status"],
            "unavailable"
        );
    }

    #[test]
    fn pi_absent_empty_and_unknown_declarations_never_infer_capability() {
        let (_dir, settings, models) = pi_files();
        fs::write(&settings, r#"{"defaultProvider":"p","defaultModel":"m"}"#).unwrap();
        for map in [None, Some(json!({})), Some(json!({"private-key":PRIVATE}))] {
            let mut model = json!({"id":"m"});
            if let Some(map) = map.clone() {
                model["thinkingLevelMap"] = map;
            }
            fs::write(
                &models,
                json!({"providers":{"p":{"api":PRIVATE,"models":[model]}}}).to_string(),
            )
            .unwrap();
            let v = pi_snapshot(&settings, &models);
            let d = &v["diskModels"]["data"];
            assert_eq!(d["reasoning"]["status"], "not_configured");
            assert_eq!(d["providerProtocol"]["status"], "unavailable");
            assert_eq!(
                d["thinkingLevelMap"]["status"],
                match &map {
                    None => "not_configured",
                    Some(value) if value.as_object().unwrap().is_empty() => "ok",
                    Some(_) => "unavailable",
                }
            );
            assert!(!v.to_string().contains(PRIVATE));
            assert!(!v.to_string().contains("private-key"));
        }
    }

    #[test]
    fn pi_invalid_provider_container_is_not_reported_as_missing() {
        let (_dir, settings, models) = pi_files();
        fs::write(&settings, r#"{"defaultProvider":"p","defaultModel":"m"}"#).unwrap();
        for bad in [
            r#"{"providers":[]}"#,
            r#"{"providers":null}"#,
            r#"{"providers":{"p":null}}"#,
        ] {
            fs::write(&models, bad).unwrap();
            assert_eq!(
                pi_snapshot(&settings, &models)["diskModels"]["status"],
                "unavailable"
            );
        }
        fs::write(&models, "{}").unwrap();
        assert_eq!(
            pi_snapshot(&settings, &models)["diskModels"]["status"],
            "not_configured"
        );
    }

    #[test]
    fn selected_readers_use_existing_bounded_file_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert_eq!(claude_snapshot(&path)["diskConfig"]["status"], "truncated");
        assert_eq!(
            pi_snapshot(&path, &dir.path().join("models.json"))["diskConfig"]["status"],
            "truncated"
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert_eq!(
                claude_snapshot(&link)["diskConfig"]["status"],
                "not_supported"
            );
            assert_eq!(
                pi_snapshot(&link, &path)["diskConfig"]["status"],
                "not_supported"
            );
        }
    }
}
