//! Gemini keeps dotenv and JSON separate; both are planned from one read and
//! committed by the existing shared-settings AppWrite transaction.
use super::*;
use crate::live::{
    engine,
    patch::{dotenv, Guarded, LivePatch, WholeFile},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeminiSharedDraft {
    pub env: String,
    pub settings: String,
}
fn env_key_allowed(key: &str) -> bool {
    !key.is_empty()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !crate::live::floor::gemini_floor_env(key)
        && !ProviderService::is_sensitive_config_key(key)
        && !key.eq_ignore_ascii_case("authorization")
        && !key.to_ascii_lowercase().contains("header")
}
fn safe_literal(value: &str) -> bool {
    !value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | '$' | '"' | '\'' | '`'))
}
fn env_text(bytes: Option<&[u8]>) -> Result<&str, AppError> {
    std::str::from_utf8(bytes.unwrap_or_default()).map_err(|_| shared_problem("invalidFormat"))
}
// None means the original assignment cannot safely be edited. Keep it even if
// a later duplicate is literal: a patch must not delete an unknown occurrence.
fn env_fields(text: &str) -> Result<BTreeMap<String, Option<String>>, AppError> {
    let mut fields = BTreeMap::new();
    for (key, value) in dotenv::literal_owned_occurrences(text, |_| true)
        .ok_or_else(|| shared_problem("invalidFormat"))?
    {
        let value = value.filter(|value| safe_literal(value));
        if !fields.get(&key).is_some_and(Option::is_none) {
            fields.insert(key, value);
        }
    }
    Ok(fields)
}
fn editable_env(fields: &BTreeMap<String, Option<String>>) -> BTreeMap<String, String> {
    fields
        .iter()
        .filter(|(key, _)| env_key_allowed(key))
        .filter_map(|(key, value)| value.as_ref().map(|value| (key.clone(), value.clone())))
        .collect()
}
fn env_config(fields: &BTreeMap<String, String>) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{key}={}\n", Value::String(value.clone())))
        .collect()
}
fn env_draft(text: &str) -> Result<BTreeMap<String, String>, AppError> {
    // Strict syntax validation does not define the value semantics; the original
    // literal reader does. Do not expose the strict parser's source-line errors.
    let parsed = crate::gemini_config::parse_env_file_strict(text)
        .map_err(|_| shared_problem("invalidFormat"))?;
    let fields = env_fields(text)?;
    if fields.len() != parsed.len() || fields.keys().any(|key| !parsed.contains_key(key)) {
        return Err(shared_problem("invalidFormat"));
    }
    fields
        .into_iter()
        .map(|(key, value)| {
            if !env_key_allowed(&key) {
                return Err(shared_problem("readOnlyFields"));
            }
            Ok((key, value.ok_or_else(|| shared_problem("invalidFormat"))?))
        })
        .collect()
}
fn json_allowed(path: &[String], _value: &Value) -> bool {
    if path.iter().any(|key| {
        ProviderService::is_sensitive_config_key(key)
            || matches!(
                key.to_ascii_lowercase().as_str(),
                "authorization"
                    | "headers"
                    | "http_headers"
                    | "httpheaders"
                    | "env_http_headers"
                    | "envhttpheaders"
            )
    }) {
        return false;
    }
    if path.first().is_some_and(|key| {
        matches!(
            key.as_str(),
            "mcp"
                | "mcpServers"
                | "mcp_servers"
                | "enabledMcpjsonServers"
                | "disabledMcpjsonServers"
                | "enableAllProjectMcpServers"
                | "allowedMcpServers"
                | "deniedMcpServers"
        )
    }) {
        return false;
    }
    if path.starts_with(&["security".into(), "auth".into()]) {
        return false;
    }
    !crate::live::floor::GEMINI_FLOOR_SETTINGS
        .iter()
        .any(|owned| {
            path.len() >= owned.len() && path.iter().zip(*owned).all(|(part, owned)| part == owned)
        })
}
fn settings_config(source: &SharedSource) -> Result<String, AppError> {
    let mut value = claude_shared_parse(&source.file.path, source.pre.as_deref())?;
    claude_shared_projection(&mut value, &[], json_allowed);
    serde_json::to_string_pretty(&value).map_err(|_| shared_problem("invalidFormat"))
}
pub(super) fn read(sources: &[SharedSource]) -> Result<GeminiSharedDraft, AppError> {
    Ok(GeminiSharedDraft {
        env: env_config(&editable_env(&env_fields(env_text(
            sources[0].pre.as_deref(),
        )?)?)),
        settings: settings_config(&sources[1])?,
    })
}
pub(super) fn plan(
    draft: &GeminiSharedDraft,
    id: &str,
    sources: Vec<SharedSource>,
) -> Result<SharedPlan, AppError> {
    let encoded = serde_json::to_string(draft).map_err(|_| shared_problem("invalidFormat"))?;
    let mut request = shared_request(SharedApp::Gemini, &encoded, id)?;
    let revision: Vec<_> = sources
        .iter()
        .map(|source| (&source.file.path, engine::digest(source.pre.as_deref())))
        .collect();
    request.revision = engine::sha256_hex(
        &serde_json::to_vec(&(SharedApp::Gemini.scope(), revision))
            .map_err(|_| shared_problem("invalidFormat"))?,
    );
    let full = env_fields(env_text(sources[0].pre.as_deref())?)?;
    let old = editable_env(&full);
    let new = env_draft(&draft.env)?;
    let mut patch = dotenv::DotenvPatch::default();
    let mut changes = Vec::new();
    for (key, value) in &new {
        if old.get(key) == Some(value) {
            continue;
        }
        if full.get(key).is_some_and(Option::is_none) {
            return Err(shared_problem("readOnlyFields"));
        }
        patch
            .set
            .push((key.clone(), Value::String(value.clone()).to_string()));
        let mut change = serde_json::json!({"role":"geminiEnv","field":key,"after":Value::String(value.clone()).to_string()});
        if let Some(before) = old.get(key) {
            change["before"] = serde_json::json!(Value::String(before.clone()).to_string());
        }
        changes.push(change);
    }
    for (key, value) in old.iter().filter(|(key, _)| !new.contains_key(*key)) {
        patch.remove.push(key.clone());
        changes.push(serde_json::json!({"role":"geminiEnv","field":key,"before":Value::String(value.clone()).to_string()}));
    }
    let source = &sources[0];
    let then = if changes.is_empty() {
        source
            .pre
            .as_ref()
            .map_or(WholeFile::Delete, |bytes| WholeFile::Write(bytes.clone()))
    } else {
        WholeFile::Write(
            patch
                .apply(&source.file.path, source.pre.as_deref())
                .map_err(|_| shared_problem("invalidFormat"))?,
        )
    };
    let env_patch = Guarded {
        expected_pre: engine::digest(source.pre.as_deref()),
        then,
    };
    let source = &sources[1];
    let (settings_patch, mut settings_changes) = shared_json_patch(
        &source.file,
        source.pre.as_deref(),
        &draft.settings,
        json_allowed,
        true,
    )?;
    for change in &mut settings_changes {
        change["role"] = serde_json::json!("geminiSettings");
    }
    changes.extend(settings_changes);
    let mut sources = sources.into_iter();
    let env = sources.next().unwrap();
    let settings = sources.next().unwrap();
    Ok(SharedPlan {
        request,
        files: vec![
            (env.role, env.file, env_patch),
            (settings.role, settings.file, settings_patch),
        ],
        changes,
    })
}
fn legacy_fields(state: &crate::store::AppState) -> Result<BTreeMap<String, String>, AppError> {
    let Some(text) = shared_import_snippet(SharedApp::Gemini, state)? else {
        return Ok(Default::default());
    };
    let value: Value = serde_json::from_str(&text).map_err(|_| shared_problem("invalidFormat"))?;
    let map = value
        .as_object()
        .ok_or_else(|| shared_problem("invalidFormat"))?;
    // The historical Gemini fragment is an env dictionary, never settings.json.
    Ok(map
        .iter()
        .filter(|(key, _)| env_key_allowed(key))
        .filter_map(|(key, value)| {
            value
                .as_str()
                .filter(|value| safe_literal(value))
                .map(|value| (key.clone(), value.into()))
        })
        .collect())
}
pub(super) fn import_fields(
    state: &crate::store::AppState,
    sources: &[SharedSource],
) -> Result<Vec<Value>, AppError> {
    let existing = env_fields(env_text(sources[0].pre.as_deref())?)?;
    let _settings = settings_config(&sources[1])?;
    Ok(legacy_fields(state)?.into_iter().filter(|(key,_)|!existing.contains_key(key)).map(|(key,value)| {
        serde_json::json!({"field":key,"path":[key],"value":Value::String(value).to_string()})
    }).collect())
}
pub(super) fn seed(
    state: &crate::store::AppState,
    sources: &[SharedSource],
    fields: &[Vec<String>],
) -> Result<GeminiSharedDraft, AppError> {
    let existing = env_fields(env_text(sources[0].pre.as_deref())?)?;
    let mut values = editable_env(&existing);
    let source = legacy_fields(state)?;
    for path in fields.iter().collect::<std::collections::BTreeSet<_>>() {
        let [key] = path.as_slice() else {
            return Err(shared_problem("readOnlyFields"));
        };
        let value = source
            .get(key)
            .ok_or_else(|| shared_problem("readOnlyFields"))?;
        if !existing.contains_key(key) {
            values.insert(key.clone(), value.clone());
        }
    }
    Ok(GeminiSharedDraft {
        env: env_config(&values),
        settings: settings_config(&sources[1])?,
    })
}
