//! U02 form adapter. Private plans, writes and outcomes stay with their original owners.
use super::ProviderService;
use crate::{
    app_config::AppType,
    database::Database,
    error::AppError,
    live::engine::{digest, DeviceStore},
    mode::{
        controller, operation,
        state::{self, SaveRequest},
    },
    provider::Provider,
    store::AppState,
};
use serde_json::{json, Value};

fn problem(code: &str) -> AppError {
    AppError::InvalidInput(format!("provider.edit.{code}"))
}
fn code(error: &AppError) -> &'static str {
    match error {
        AppError::InvalidInput(value) if value == "provider.edit.credentialConflict" => {
            "credentialConflict"
        }
        AppError::InvalidInput(value) if value == "provider.edit.readOnly" => "readOnly",
        AppError::InvalidInput(value) if value == "provider.edit.pendingOperation" => {
            "pendingOperation"
        }
        AppError::Config(value) if value.contains("locked") => "vaultLocked",
        AppError::Conflict(_) => "sourceChanged",
        AppError::Io { .. }
        | AppError::IoContext { .. }
        | AppError::Lock(_)
        | AppError::Database(_) => "ownerUnavailable",
        AppError::Config(value)
            if value == "mode.verification_required"
                || value.starts_with("upgrade.")
                || value == "codex.auth_store_unavailable" =>
        {
            "ownerUnavailable"
        }
        AppError::Localized { key, .. }
            if key.contains("api_key") || key.ends_with(".credentials.missing") =>
        {
            "credentialConflict"
        }
        _ => "invalidFormat",
    }
}
fn admitted(state: &AppState, app: &AppType) -> Result<(), AppError> {
    if !controller::PROXY_APPS.contains(app) || !operation::uses_upstream4_schema(&state.db)? {
        return Err(problem("readOnly"));
    }
    Ok(())
}
fn draft_digest(
    app: &AppType,
    provider: &Provider,
    original: &str,
    delete: bool,
) -> Result<String, AppError> {
    crate::config::serialize_json_bytes(&json!([app.as_str(), original, provider, delete]))
        .map(|bytes| digest(Some(&bytes)).unwrap())
}
fn result(app: &AppType, request: &SaveRequest, status: &str, why: Option<&str>) -> Value {
    let mut value = json!({"app":app.as_str(),"request":request,"status":status});
    if let Some(code) = why {
        value["code"] = json!(code);
    }
    value
}
fn query_locked(state: &AppState, app: &AppType, request: &SaveRequest) -> Result<Value, AppError> {
    admitted(state, app)?;
    let vault = state.db.secret_session().read()?;
    let live = state::load_app(&DeviceStore::for_device(), &vault, app.as_str())?;
    let Some(entry) = live.apps.get(app.as_str()) else {
        return Ok(result(app, request, "notRecorded", None));
    };
    state::validate_app_evidence_for_update(app.as_str(), entry)?;
    if let Some(pending) = &entry.pending {
        if let Some(original) = &pending.target.save_request {
            let view = operation::original_save_view(app.as_str(), entry, &original.provider_id)?
                .ok_or_else(|| problem("ownerUnavailable"))?;
            if original.id == request.id {
                return Ok(result(
                    app,
                    request,
                    if original != request {
                        "conflict"
                    } else {
                        view.status
                    },
                    None,
                ));
            }
        }
    }
    if let Some(receipt) = &entry.last_save {
        if receipt.request.id == request.id {
            let status = if receipt.request != *request {
                "conflict"
            } else {
                match receipt.outcome {
                    state::SaveOutcome::Completed => "completed",
                    state::SaveOutcome::Discarded => "discarded",
                    state::SaveOutcome::Abandoned => "abandoned",
                }
            };
            return Ok(result(app, request, status, None));
        }
        return Ok(result(app, request, "unknown", None));
    }
    Ok(result(app, request, "notRecorded", None))
}
pub(crate) fn query(
    state: &AppState,
    app: &AppType,
    request: SaveRequest,
) -> Result<Value, AppError> {
    request.validate()?;
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    Ok(query_locked(state, app, &request)
        .unwrap_or_else(|error| result(app, &request, "unknown", Some(code(&error)))))
}

fn key_text(value: &str) -> Result<Option<&str>, AppError> {
    let value = value.trim();
    if value.contains("***") || value.contains('•') || value == "[REDACTED]" {
        return Err(problem("credentialConflict"));
    }
    Ok((!value.is_empty()).then_some(value))
}
fn key(value: Option<&Value>) -> Result<Option<&str>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => key_text(value),
        _ => Err(problem("invalidFormat")),
    }
}
fn json_keys(
    previous: &Value,
    draft: &mut Value,
    paths: &[&str],
    delete: bool,
) -> Result<(), AppError> {
    for path in paths {
        let before = key(previous.pointer(path))?;
        let after = key(draft.pointer(path))?;
        if delete && after.is_some() && after != before {
            return Err(problem("credentialConflict"));
        }
        let (parent, name) = path.rsplit_once('/').unwrap();
        if delete {
            if let Some(map) = draft.pointer_mut(parent).and_then(Value::as_object_mut) {
                map.remove(name);
            }
        } else if after.is_none() {
            if let Some(original) = previous.pointer(path).filter(|_| before.is_some()) {
                if !parent.is_empty() && draft.pointer(parent).is_none() {
                    draft[parent.trim_start_matches('/')] = json!({});
                }
                draft
                    .pointer_mut(parent)
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| problem("invalidFormat"))?
                    .insert(name.into(), original.clone());
            }
        }
    }
    let mut values = paths
        .iter()
        .map(|path| key(draft.pointer(path)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    if values.len() > 1 {
        return Err(problem("credentialConflict"));
    }
    Ok(())
}
fn prepare(
    state: &AppState,
    app: &AppType,
    previous: &Provider,
    mut draft: Provider,
    delete: bool,
) -> Result<Provider, AppError> {
    if previous.id != draft.id {
        return Err(problem("readOnly"));
    }
    if crate::relay::is_managed(&previous.id) && delete {
        return Err(problem("readOnly"));
    }
    match app {
        AppType::Claude => json_keys(
            &previous.settings_config,
            &mut draft.settings_config,
            &[
                "/apiKey",
                "/env/ANTHROPIC_AUTH_TOKEN",
                "/env/ANTHROPIC_API_KEY",
            ],
            delete,
        )?,
        AppType::Gemini => json_keys(
            &previous.settings_config,
            &mut draft.settings_config,
            &["/env/GEMINI_API_KEY", "/env/GOOGLE_API_KEY"],
            delete,
        )?,
        AppType::Codex => {
            json_keys(
                &previous.settings_config,
                &mut draft.settings_config,
                &["/auth/OPENAI_API_KEY"],
                delete,
            )?;
            let before = previous
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or("");
            let text = draft
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or("");
            let old_doc = before
                .parse::<toml_edit::DocumentMut>()
                .map_err(|_| problem("invalidFormat"))?;
            let mut doc = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|_| problem("invalidFormat"))?;
            let selected = crate::codex_config::active_codex_model_provider_id(&doc);
            if !delete
                && selected != crate::codex_config::active_codex_model_provider_id(&old_doc)
                && crate::codex_config::extract_codex_experimental_bearer_token(text).is_none()
                && crate::codex_config::extract_codex_experimental_bearer_token(before).is_some()
            {
                // Blank preservation cannot move a credential to a different table.
                return Err(problem("credentialConflict"));
            }
            fn slot<'a>(
                doc: &'a toml_edit::DocumentMut,
                selected: Option<&str>,
            ) -> Option<&'a toml_edit::Item> {
                match selected {
                    Some(id) => doc
                        .get("model_providers")?
                        .get(id)?
                        .get("experimental_bearer_token"),
                    None => doc.get("experimental_bearer_token"),
                }
            }
            fn token(item: Option<&toml_edit::Item>) -> Result<Option<&str>, AppError> {
                match item {
                    None => Ok(None),
                    Some(item) => key_text(item.as_str().ok_or_else(|| problem("invalidFormat"))?),
                }
            }
            // Validate exactly the two slots the existing removal owner touches.
            // Inactive tables remain untouched, including their credentials.
            let mut slots = vec![None];
            if let Some(id) = selected.as_deref() {
                slots.push(Some(id));
            }
            for selected in &slots {
                let before = token(slot(&old_doc, *selected))?;
                let after = token(slot(&doc, *selected))?;
                if delete && after.is_some() && after != before {
                    return Err(problem("credentialConflict"));
                }
                if !delete && after.is_none() && before.is_some() {
                    let original = slot(&old_doc, *selected).unwrap().clone();
                    if let Some(id) = selected {
                        doc.get_mut("model_providers")
                            .and_then(|v| v.get_mut(id))
                            .and_then(toml_edit::Item::as_table_like_mut)
                            .ok_or_else(|| problem("credentialConflict"))?
                            .insert("experimental_bearer_token", original);
                    } else {
                        doc.insert("experimental_bearer_token", original);
                    }
                }
            }
            if delete {
                draft.settings_config["config"] = json!(
                    crate::codex_config::remove_codex_experimental_bearer_token_if(text, |_| true)?
                );
            } else {
                let mut values = slots
                    .iter()
                    .map(|selected| token(slot(&doc, *selected)))
                    .collect::<Result<Vec<_>, _>>()?;
                values.push(key(draft.settings_config.pointer("/auth/OPENAI_API_KEY"))?);
                let mut values = values.into_iter().flatten().collect::<Vec<_>>();
                values.sort();
                values.dedup();
                if values.len() > 1 {
                    return Err(problem("credentialConflict"));
                }
                draft.settings_config["config"] = json!(doc.to_string());
            }
        }
        AppType::GrokBuild => {
            let previous = previous
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or("");
            let text = draft
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .ok_or_else(|| problem("invalidFormat"))?;
            let before = crate::grok_config::extract_inline_api_key(previous);
            let after = crate::grok_config::extract_inline_api_key(text);
            if let Some(after) = &after {
                key(Some(&json!(after)))?;
            }
            if delete && after.is_some() && after != before {
                return Err(problem("credentialConflict"));
            }
            if delete {
                draft.settings_config["config"] = json!(
                    crate::grok_config::remove_selected_model_string(text, "api_key")?
                );
            } else if after.as_deref().is_none_or(|value| value.trim().is_empty()) {
                if let Some(before) = before {
                    draft.settings_config["config"] =
                        json!(crate::grok_config::update_api_key(text, &before)?);
                }
            }
        }
        _ => return Err(problem("readOnly")),
    }
    let draft =
        ProviderService::prepare_managed_update(state, app.clone(), Some(&previous.id), draft)?;
    ProviderService::prepare_provider_update(state, app, draft)
}

fn planned(
    state: &AppState,
    app: &AppType,
    draft: Provider,
    original: &str,
    id: &str,
    delete: bool,
) -> Result<(Provider, Provider, controller::SaveRowPlan), AppError> {
    admitted(state, app)?;
    if draft.id != original {
        return Err(problem("readOnly"));
    }
    {
        let vault = state.db.secret_session().read()?;
        if state::pending(&DeviceStore::for_device(), &vault, app.as_str())?.is_some() {
            return Err(problem("pendingOperation"));
        }
    }
    let previous = state
        .db
        .get_provider_by_id(original, app.as_str())?
        .ok_or_else(|| problem("readOnly"))?;
    let draft = prepare(state, app, &previous, draft, delete)?;
    let plan = controller::plan_row_save(&state.proxy_service, app, &previous, &draft, id)?;
    Ok((previous, draft, plan))
}
fn fields(previous: &Provider, next: &Provider) -> Vec<&'static str> {
    fn expanded(value: &Value) -> Value {
        let mut value = value.clone();
        if let Some(text) = value.get("config").and_then(Value::as_str) {
            if let Ok(config) = text.parse::<toml::Value>() {
                value["config"] = serde_json::to_value(config).unwrap_or(Value::Null);
            }
        }
        value
    }
    fn changed(
        a: &Value,
        b: &Value,
        path: &str,
        out: &mut std::collections::BTreeSet<&'static str>,
    ) {
        if a == b {
            return;
        }
        if a.is_object() || b.is_object() {
            let keys = a
                .as_object()
                .into_iter()
                .flat_map(|o| o.keys())
                .chain(b.as_object().into_iter().flat_map(|o| o.keys()))
                .collect::<std::collections::BTreeSet<_>>();
            for key in keys {
                changed(&a[key], &b[key], &format!("{path}/{key}"), out);
            }
        } else {
            let path = path.to_ascii_lowercase();
            out.insert(
                if path.contains("auth")
                    || path.contains("api_key")
                    || path.contains("apikey")
                    || path.contains("token")
                {
                    "authentication"
                } else if path.contains("base_url")
                    || path.contains("baseurl")
                    || path.contains("endpoint")
                    || path.contains("wire_api")
                {
                    "connection"
                } else if path.contains("model") {
                    "models"
                } else {
                    "dedicated"
                },
            );
        }
    }
    let mut out = std::collections::BTreeSet::new();
    changed(
        &expanded(&previous.settings_config),
        &expanded(&next.settings_config),
        "",
        &mut out,
    );
    let mut old = previous.clone();
    old.settings_config = next.settings_config.clone();
    if Database::provider_update_digest(&old).ok() != Database::provider_update_digest(next).ok() {
        out.insert("metadata");
    }
    out.into_iter().collect()
}

pub(crate) fn preview(
    state: &AppState,
    app: &AppType,
    provider: Provider,
    original: &str,
    id: &str,
    delete_credential: bool,
) -> Result<Value, AppError> {
    let draft = draft_digest(app, &provider, original, delete_credential)?;
    let mut request = SaveRequest {
        id: id.into(),
        provider_id: original.into(),
        draft_digest: draft.clone(),
        revision: draft,
    };
    request.validate()?;
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    let mut value = json!({"app":app.as_str(), "request": request, "status":"blocked", "action":"saveOnly", "fields":[], "files":[], "preserves":[]});
    match planned(state, app, provider, original, id, delete_credential) {
        Ok((previous, provider, plan)) => {
            request.revision = plan.revision;
            let roles: &[&str] = match app {
                AppType::Claude => &["claudeSettings"],
                AppType::Codex => &[
                    "codexAuth",
                    "codexConfig",
                    "codexCatalog",
                    "managedAuth",
                    "deviceAuthStash",
                ],
                AppType::Gemini => &["geminiEnv", "geminiSettings"],
                AppType::GrokBuild => &["grokConfig"],
                _ => &[],
            };
            let files = plan.files.iter().zip(roles).map(|((_, patch), role)| {
                let after = match &patch.then { crate::live::patch::WholeFile::Write(bytes) => digest(Some(bytes)), crate::live::patch::WholeFile::Delete => None };
                json!({"role":role,"change":if after == patch.expected_pre {"unchanged"} else if after.is_none() {"delete"} else {"write"}})
            }).collect::<Vec<_>>();
            let mut preserves = vec!["sharedSettings"];
            match app {
                AppType::Claude => preserves.push("unownedJsonKeys"),
                AppType::Gemini => preserves.extend(["unownedJsonKeys", "untouchedDotenvBytes"]),
                _ => preserves.push("untouchedTomlBytes"),
            }
            if plan.preserved_catalog {
                preserves.push("externalCatalog");
            }
            value = json!({"app":app.as_str(),"request":request,"status":"ready","action":if plan.files.is_empty(){"saveOnly"}else{"saveAndApply"},"fields":fields(&previous,&provider),"files":files,"preserves":preserves});
        }
        Err(error) => value["code"] = json!(code(&error)),
    }
    Ok(value)
}

pub(crate) fn confirm(
    state: &AppState,
    app: &AppType,
    provider: Provider,
    original: &str,
    request: SaveRequest,
    delete_credential: bool,
) -> Result<Value, AppError> {
    request.validate()?;
    if request.provider_id != original
        || provider.id != original
        || draft_digest(app, &provider, original, delete_credential)? != request.draft_digest
    {
        return Ok(result(app, &request, "conflict", None));
    }
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    let old = match query_locked(state, app, &request) {
        Ok(value) => value,
        Err(error) => return Ok(result(app, &request, "blocked", Some(code(&error)))),
    };
    if !matches!(old["status"].as_str(), Some("notRecorded" | "unknown")) {
        return Ok(old);
    }
    let (_, _, mut plan) = match planned(
        state,
        app,
        provider,
        original,
        &request.id,
        delete_credential,
    ) {
        Ok(plan) => plan,
        Err(error) => return Ok(result(app, &request, "blocked", Some(code(&error)))),
    };
    if plan.revision != request.revision {
        return Ok(result(app, &request, "stale", Some("sourceChanged")));
    }
    plan.target.save_request = Some(request.clone());
    match controller::consume_row_save(&state.proxy_service, app, plan, request.clone()) {
        Ok(()) => query_locked(state, app, &request),
        Err(error) => {
            let recorded = query_locked(state, app, &request)
                .unwrap_or_else(|_| result(app, &request, "unknown", None));
            if matches!(recorded["status"].as_str(), Some("notRecorded" | "unknown")) {
                Ok(result(app, &request, "blocked", Some(code(&error))))
            } else {
                Ok(recorded)
            }
        }
    }
}
