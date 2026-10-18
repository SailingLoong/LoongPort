//! Field-owned application settings. Persistence remains with `edit` and its
//! authenticated row/file transaction; this module never writes a database.
use crate::{
    app_config::AppType,
    codex_reasoning::{ReasoningIntent, ReasoningMode},
    error::AppError,
    provider::Provider,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDeclaration {
    pub reasoning_mode: ReasoningMode,
    pub reasoning_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_level: Option<String>,
}

/// A recovery observation, not an independent source of model capabilities.
/// It is published only as part of the same successful provider-row save.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedDefaults {
    pub source_identity: String,
    pub observed_revision: String,
    pub model: Option<String>,
    pub reasoning: ReasoningIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationModelOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited: Option<ManagedDefaults>,
    /// Ownership markers only. Effective facts remain in their existing fields.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub declaration_models: BTreeSet<String>,
    #[serde(default)]
    pub transport: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum FieldChange<T> {
    Set(T),
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclarationChange {
    pub model: String,
    pub change: FieldChange<ModelDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateSelection {
    pub template_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSettingsPatch {
    #[serde(default)]
    pub confirm_source_change: bool,
    pub source_change_revision: Option<String>,
    pub model: Option<FieldChange<String>>,
    pub reasoning: Option<FieldChange<ReasoningIntent>>,
    pub declaration: Option<DeclarationChange>,
    pub transport: Option<FieldChange<TemplateSelection>>,
}

fn invalid(code: &str) -> AppError {
    AppError::InvalidInput(format!("provider.modelSettings.{code}"))
}

fn codex_config(provider: &Provider) -> Result<toml_edit::DocumentMut, AppError> {
    provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("invalidConfig"))?
        .parse()
        .map_err(|_| invalid("invalidConfig"))
}

fn legacy_intent(provider: &Provider) -> Result<ReasoningIntent, AppError> {
    let config = codex_config(provider)?;
    Ok(
        match config
            .get("model_reasoning_effort")
            .and_then(toml_edit::Item::as_str)
        {
            None => ReasoningIntent::ServiceDefault,
            Some("none") => ReasoningIntent::Off,
            Some(value) => ReasoningIntent::Effort(value.into()),
        },
    )
}

fn owned(provider: &Provider) -> ApplicationModelOverrides {
    provider
        .meta
        .as_ref()
        .and_then(|m| m.application_model_overrides.clone())
        .unwrap_or_default()
}

fn account_identity(app: &AppType, provider: &Provider) -> Result<String, AppError> {
    let meta = provider.meta.as_ref();
    let value = json!([
        app.as_str(),
        provider.id,
        provider.website_url,
        meta.and_then(|m| m.loongport_account_id),
        meta.and_then(|m| m.loongport_vendor_account.as_ref()),
        meta.and_then(|m| m.loongport_group.as_ref()).map(|g| &g.id),
        meta.and_then(|m| m.auth_binding.as_ref())
    ]);
    let bytes = crate::config::serialize_json_bytes(&value)?;
    crate::live::engine::digest(Some(&bytes)).ok_or_else(|| invalid("sourceUnavailable"))
}

fn has_explicit_overrides(value: &ApplicationModelOverrides) -> bool {
    value.model.is_some()
        || value.reasoning.is_some()
        || value.transport
        || !value.declaration_models.is_empty()
}

/// A common request/catalog applicability guard, not another persisted identity.
/// An inheritance-only observation never grants or blocks request capabilities.
pub(crate) fn owned_scope_matches(provider: &Provider) -> bool {
    let value = owned(provider);
    !has_explicit_overrides(&value)
        || source_identity(&AppType::Codex, provider)
            .ok()
            .as_ref()
            .is_some_and(|actual| value.source_identity.as_ref() == Some(actual))
}

fn source_identity(app: &AppType, provider: &Provider) -> Result<String, AppError> {
    let meta = provider.meta.as_ref();
    // No credential, model choice or capability declaration belongs to source identity.
    let config = codex_config(provider)?;
    let selected_provider = config
        .get("model_provider")
        .and_then(toml_edit::Item::as_str);
    let endpoint = selected_provider.and_then(|id| {
        config
            .get("model_providers")
            .and_then(|providers| providers.get(id))
    });
    let value = json!([
        app.as_str(),
        provider.id,
        provider.website_url,
        meta.and_then(|m| m.loongport_account_id),
        meta.and_then(|m| m.loongport_vendor_account.as_ref()),
        meta.and_then(|m| m.loongport_group.as_ref()).map(|g| &g.id),
        meta.and_then(|m| m.auth_binding.as_ref()),
        selected_provider,
        endpoint
            .and_then(|v| v.get("base_url"))
            .and_then(toml_edit::Item::as_str),
        endpoint
            .and_then(|v| v.get("wire_api"))
            .and_then(toml_edit::Item::as_str),
        config
            .get("openai_base_url")
            .and_then(toml_edit::Item::as_str),
        config.get("base_url").and_then(toml_edit::Item::as_str),
        provider.settings_config.get("base_url"),
        meta.and_then(|m| m.api_format.as_ref())
    ]);
    let bytes = crate::config::serialize_json_bytes(&value)?;
    crate::live::engine::digest(Some(&bytes)).ok_or_else(|| invalid("sourceUnavailable"))
}

fn declaration(provider: &Provider, model: &str) -> Result<Option<ModelDeclaration>, AppError> {
    let Some(row) = provider
        .settings_config
        .pointer("/modelCatalog/models")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("model").and_then(Value::as_str) == Some(model))
        })
    else {
        return Ok(None);
    };
    if row.get("reasoningMode").is_none()
        && row.get("reasoningLevels").is_none()
        && row.get("defaultReasoningLevel").is_none()
    {
        return Ok(None);
    }
    // Legacy rows can omit a mode; keep that distinction during restoration.
    let mode = row.get("reasoningMode").cloned().unwrap_or_else(|| {
        json!(crate::codex_reasoning::declared_capabilities(row)
            .map(|c| c.mode)
            .unwrap_or_default())
    });
    serde_json::from_value(json!({"reasoningMode":mode,
        "reasoningLevels": row.get("reasoningLevels").cloned().unwrap_or(json!([])),
        "defaultReasoningLevel":row.get("defaultReasoningLevel").cloned().unwrap_or(Value::Null)}))
    .map(Some)
    .map_err(|_| invalid("invalidDeclaration"))
}

fn declaration_image(provider: &Provider, model: &str) -> Option<Value> {
    let row = provider
        .settings_config
        .pointer("/modelCatalog/models")?
        .as_array()?
        .iter()
        .find(|row| row.get("model").and_then(Value::as_str) == Some(model))?;
    let mut image = serde_json::Map::new();
    for key in ["reasoningMode", "reasoningLevels", "defaultReasoningLevel"] {
        if let Some(value) = row.get(key) {
            image.insert(key.into(), value.clone());
        }
    }
    (!image.is_empty()).then_some(Value::Object(image))
}

fn restore_declaration(
    provider: &mut Provider,
    model: &str,
    image: Option<&Value>,
) -> Result<(), AppError> {
    write_declaration(provider, model, None)?;
    let row = provider
        .settings_config
        .pointer_mut("/modelCatalog/models")
        .and_then(Value::as_array_mut)
        .and_then(|rows| {
            rows.iter_mut()
                .find(|row| row.get("model").and_then(Value::as_str) == Some(model))
        })
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid("invalidCatalog"))?;
    if let Some(image) = image {
        for (key, value) in image
            .as_object()
            .ok_or_else(|| invalid("invalidDeclaration"))?
        {
            if !["reasoningMode", "reasoningLevels", "defaultReasoningLevel"]
                .contains(&key.as_str())
            {
                return Err(invalid("invalidDeclaration"));
            }
            row.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn write_declaration(
    provider: &mut Provider,
    model: &str,
    value: Option<&ModelDeclaration>,
) -> Result<(), AppError> {
    if value.is_none()
        && provider
            .settings_config
            .pointer("/modelCatalog/models")
            .and_then(Value::as_array)
            .is_none_or(|rows| {
                !rows
                    .iter()
                    .any(|row| row.get("model").and_then(Value::as_str) == Some(model))
            })
    {
        return Ok(());
    }
    let settings = provider
        .settings_config
        .as_object_mut()
        .ok_or_else(|| invalid("invalidConfig"))?;
    let catalog = settings
        .entry("modelCatalog")
        .or_insert_with(|| json!({"models":[]}));
    let rows = catalog
        .as_object_mut()
        .ok_or_else(|| invalid("invalidCatalog"))?
        .entry("models")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| invalid("invalidCatalog"))?;
    let positions: Vec<_> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| (r.get("model").and_then(Value::as_str) == Some(model)).then_some(i))
        .collect();
    if positions.len() > 1 {
        return Err(invalid("duplicateModel"));
    }
    let i = match positions.first() {
        Some(i) => *i,
        None => {
            rows.push(json!({"model":model}));
            rows.len() - 1
        }
    };
    let row = rows[i]
        .as_object_mut()
        .ok_or_else(|| invalid("invalidCatalog"))?;
    for key in ["reasoningMode", "reasoningLevels", "defaultReasoningLevel"] {
        row.remove(key);
    }
    if let Some(value) = value {
        let fields = serde_json::to_value(value).map_err(|_| invalid("invalidDeclaration"))?;
        row.extend(
            fields
                .as_object()
                .ok_or_else(|| invalid("invalidDeclaration"))?
                .clone(),
        );
    }
    Ok(())
}

fn project(
    provider: &mut Provider,
    model: Option<&str>,
    intent: Option<&ReasoningIntent>,
) -> Result<(), AppError> {
    if model.is_none() && intent.is_none() {
        return Ok(());
    }
    let mut config = codex_config(provider)?;
    if let Some(model) = model {
        config["model"] = toml_edit::value(model);
    }
    if let Some(intent) = intent {
        match intent {
            ReasoningIntent::ServiceDefault | ReasoningIntent::On => {
                config.remove("model_reasoning_effort");
            }
            ReasoningIntent::Off => {
                config["model_reasoning_effort"] = toml_edit::value("none");
            }
            ReasoningIntent::Effort(level) => {
                config["model_reasoning_effort"] = toml_edit::value(level);
            }
        }
    }
    provider.settings_config["config"] = json!(config.to_string());
    Ok(())
}

fn clear_owned_bindings(
    provider: &mut Provider,
    value: &ApplicationModelOverrides,
) -> Result<(), AppError> {
    for model in &value.declaration_models {
        write_declaration(provider, model, None)?;
    }
    let meta = provider.meta.get_or_insert_with(Default::default);
    if value.transport {
        meta.codex_chat_reasoning = None;
    }
    meta.application_model_overrides = Some(ApplicationModelOverrides::default());
    Ok(())
}

/// Construct one complete row image. The caller submits it to the existing
/// preview/confirm owner; no part is committed independently.
pub(crate) fn apply_patch(
    app: &AppType,
    previous: &Provider,
    patch: &ModelSettingsPatch,
) -> Result<Provider, AppError> {
    if *app != AppType::Codex {
        return Err(invalid("unsupportedApplication"));
    }
    let identity = source_identity(app, previous)?;
    let mut overrides = owned(previous);
    let source_changed = overrides
        .source_identity
        .as_ref()
        .is_some_and(|old| old != &identity)
        || (has_explicit_overrides(&overrides) && overrides.source_identity.is_none());
    let mut next = previous.clone();
    if patch.confirm_source_change {
        if !source_changed
            || !matches!(patch.model, Some(FieldChange::Set(_)))
            || !matches!(patch.reasoning, Some(FieldChange::Set(_)))
        {
            return Err(invalid("sourceConfirmationRequired"));
        }
        if patch.source_change_revision.as_deref()
            != Some(crate::database::Database::provider_update_digest(previous)?.as_str())
        {
            return Err(invalid("sourceChanged"));
        }
        clear_owned_bindings(&mut next, &overrides)?;
        overrides = ApplicationModelOverrides::default();
    } else if source_changed {
        return Err(invalid("sourceChanged"));
    } else if patch.source_change_revision.is_some() {
        return Err(invalid("sourceConfirmationRequired"));
    }
    overrides.source_identity = Some(identity.clone());
    let inherited = overrides
        .inherited
        .clone()
        .filter(|value| value.source_identity == identity && !value.observed_revision.is_empty());
    let mut selected =
        crate::relay::provider_config::selected_model(app, &previous.settings_config);
    let mut intent = overrides
        .reasoning
        .clone()
        .unwrap_or(legacy_intent(previous)?);
    if let Some(change) = &patch.model {
        match change {
            FieldChange::Set(model) => {
                if model.trim().is_empty()
                    || model != model.trim()
                    || model.chars().any(char::is_control)
                {
                    return Err(invalid("invalidModel"));
                }
                selected = Some(model.clone());
                overrides.model = Some(model.clone());
            }
            FieldChange::Restore => {
                selected = inherited
                    .as_ref()
                    .ok_or_else(|| invalid("restoreUnavailable"))?
                    .model
                    .clone();
                if selected.is_none() {
                    return Err(invalid("restoreUnavailable"));
                }
                overrides.model = None;
            }
        }
    }
    let selected = selected.ok_or_else(|| invalid("modelRequired"))?;
    if let Some(change) = &patch.transport {
        match change {
            FieldChange::Set(selection) => {
                crate::codex_config::validate_reasoning_template_for_provider(
                    previous,
                    &selection.template_id,
                )
                .map_err(|_| invalid("templateProtocolMismatch"))?;
                let template = crate::codex_reasoning::reasoning_template(&selection.template_id)
                    .ok_or_else(|| invalid("unknownTemplate"))?;
                if !overrides.transport {
                    overrides.transport = true;
                }
                next.meta
                    .get_or_insert_with(Default::default)
                    .codex_chat_reasoning = Some(template);
            }
            FieldChange::Restore => {
                if !overrides.transport {
                    return Err(invalid("restoreUnavailable"));
                }
                overrides.transport = false;
                next.meta
                    .get_or_insert_with(Default::default)
                    .codex_chat_reasoning = None;
            }
        }
    }
    if let Some(change) = &patch.declaration {
        if change.model != selected {
            return Err(invalid("declarationModelMismatch"));
        }
        match &change.change {
            FieldChange::Set(value) => {
                if crate::codex_reasoning::canonical_levels(&value.reasoning_levels)
                    != value.reasoning_levels
                    || value
                        .default_reasoning_level
                        .as_ref()
                        .is_some_and(|v| !value.reasoning_levels.contains(v))
                {
                    return Err(invalid("invalidDeclaration"));
                }
                overrides.declaration_models.insert(selected.clone());
                write_declaration(&mut next, &selected, Some(value))?;
            }
            FieldChange::Restore => {
                if !overrides.declaration_models.remove(&selected) {
                    return Err(invalid("restoreUnavailable"));
                }
                write_declaration(&mut next, &selected, None)?;
            }
        }
    }
    if let Some(change) = &patch.reasoning {
        match change {
            FieldChange::Set(value) => {
                intent = value.clone();
                overrides.reasoning = Some(value.clone());
            }
            FieldChange::Restore => {
                intent = inherited
                    .as_ref()
                    .ok_or_else(|| invalid("restoreUnavailable"))?
                    .reasoning
                    .clone();
                overrides.reasoning = None;
            }
        }
    }
    // Validate the proposed binding, not the stale metadata still present in the
    // input row. The whole proposed row is published only by the original owner.
    next.meta
        .get_or_insert_with(Default::default)
        .application_model_overrides = Some(overrides.clone());
    let capabilities = crate::codex_config::resolve_provider_model_capability(&next, &selected);
    if !capabilities.conflicts.is_empty()
        && (patch.declaration.is_some() || patch.transport.is_some())
    {
        return Err(invalid("capabilityConflict"));
    }
    // An untouched legacy value is preserved. Changing its model or reasoning
    // requires a supported choice rather than silently upgrading to a default.
    if patch.model.is_some()
        || patch.reasoning.is_some()
        || patch.declaration.is_some()
        || patch.transport.is_some()
    {
        capabilities
            .validate_intent(&intent)
            .map_err(|_| invalid("unsupportedReasoning"))?;
    }
    project(&mut next, Some(&selected), Some(&intent))?;
    next.meta
        .get_or_insert_with(Default::default)
        .application_model_overrides = Some(overrides);
    Ok(next)
}

/// Reconstruct the only legal narrow edit from its owned field deltas. Comparing
/// the entire reconstructed row makes URL, credential, group, other catalog-row
/// and arbitrary metadata changes fail even with a forged narrow SaveScope.
pub(crate) fn validate_change(
    app: &AppType,
    previous: &Provider,
    draft: &Provider,
) -> Result<(), AppError> {
    let original_before = owned(previous);
    let after = owned(draft);
    let identity = source_identity(app, previous)?;
    let rebind = original_before.source_identity.as_ref() != Some(&identity)
        && after.source_identity.as_ref() == Some(&identity)
        && (has_explicit_overrides(&original_before) || original_before.source_identity.is_some());
    let mut comparison = previous.clone();
    if rebind {
        clear_owned_bindings(&mut comparison, &original_before)?;
    }
    let before = owned(&comparison);
    let selected = crate::relay::provider_config::selected_model(app, &draft.settings_config)
        .ok_or_else(|| invalid("modelRequired"))?;
    let mut patch = ModelSettingsPatch {
        confirm_source_change: rebind,
        source_change_revision: if rebind {
            Some(crate::database::Database::provider_update_digest(previous)?)
        } else {
            None
        },
        ..Default::default()
    };
    if rebind || before.model != after.model {
        patch.model = Some(match &after.model {
            Some(v) => FieldChange::Set(v.clone()),
            None => FieldChange::Restore,
        });
    }
    if rebind || before.reasoning != after.reasoning {
        patch.reasoning = Some(match &after.reasoning {
            Some(v) => FieldChange::Set(v.clone()),
            None => FieldChange::Restore,
        });
    }
    if declaration_image(&comparison, &selected) != declaration_image(draft, &selected)
        || before.declaration_models != after.declaration_models
    {
        patch.declaration = Some(DeclarationChange {
            model: selected.clone(),
            change: if after.declaration_models.contains(&selected) {
                FieldChange::Set(
                    declaration(draft, &selected)?.ok_or_else(|| invalid("invalidDeclaration"))?,
                )
            } else {
                FieldChange::Restore
            },
        });
    }
    let previous_transport = comparison
        .meta
        .as_ref()
        .and_then(|m| m.codex_chat_reasoning.as_ref());
    let draft_transport = draft
        .meta
        .as_ref()
        .and_then(|m| m.codex_chat_reasoning.as_ref());
    if previous_transport != draft_transport || before.transport != after.transport {
        patch.transport = Some(if after.transport {
            let id = draft_transport
                .and_then(crate::codex_reasoning::reasoning_template_id)
                .ok_or_else(|| invalid("unknownTemplate"))?;
            FieldChange::Set(TemplateSelection {
                template_id: id.into(),
            })
        } else {
            FieldChange::Restore
        });
    }
    let expected = apply_patch(app, previous, &patch)?;
    if expected.available_models != draft.available_models
        || serde_json::to_value(&expected).map_err(|_| invalid("invalidRow"))?
            != serde_json::to_value(draft).map_err(|_| invalid("invalidRow"))?
    {
        return Err(invalid("outsideFieldScope"));
    }
    Ok(())
}

/// `generated` is the existing source owner's freshly generated default row,
/// before its legacy manual-config preservation. `next` is that owner's final
/// row. Both the observation and overrides are persisted by its one row save.
pub(crate) fn merge_managed_refresh(
    app: &AppType,
    previous: Option<&Provider>,
    generated: &Provider,
    next: &mut Provider,
) -> Result<(), AppError> {
    if *app != AppType::Codex {
        return Ok(());
    }
    let account = account_identity(app, generated)?;
    if account_identity(app, next)? != account
        || previous
            .map(|old| account_identity(app, old))
            .transpose()?
            .is_some_and(|old| old != account)
    {
        return Err(invalid("sourceChanged"));
    }
    let identity = source_identity(app, next)?;
    let mut overrides = previous.map(owned).unwrap_or_default();
    let explicit = has_explicit_overrides(&overrides);
    let previous_scope_matches = previous.is_none_or(|old| {
        source_identity(app, old).ok().as_ref() == Some(&identity) && owned_scope_matches(old)
    });
    // A valid key refresh can preserve a manual endpoint that differs from the
    // generated default. Such a default is not evidence for restoring this row.
    overrides.inherited = if source_identity(app, generated)? == identity
        && (!explicit || previous_scope_matches)
    {
        Some(ManagedDefaults {
            source_identity: identity.clone(),
            observed_revision: crate::database::Database::provider_update_digest(generated)?,
            model: crate::relay::provider_config::selected_model(app, &generated.settings_config),
            reasoning: legacy_intent(generated)?,
        })
    } else {
        None
    };
    if !explicit {
        overrides.source_identity = Some(identity);
    }
    // Keep explicit evidence and its original binding if the endpoint changed.
    // The common resolver refuses that stale binding until an explicit rebind.
    if let Some(previous) = previous {
        for model in overrides.declaration_models.iter() {
            restore_declaration(next, model, declaration_image(previous, model).as_ref())?;
        }
        if overrides.transport {
            next.meta
                .get_or_insert_with(Default::default)
                .codex_chat_reasoning = previous
                .meta
                .as_ref()
                .and_then(|m| m.codex_chat_reasoning.clone());
        }
    }
    // A changed upstream inventory is diagnostic information, not permission to
    // erase an explicit mapping. Actual request admission remains authoritative.
    if previous_scope_matches {
        project(
            next,
            overrides.model.as_deref(),
            overrides.reasoning.as_ref(),
        )?;
    }
    next.meta
        .get_or_insert_with(Default::default)
        .application_model_overrides = Some(overrides);
    Ok(())
}

pub(crate) fn save_managed_refresh(
    state: &crate::store::AppState,
    app: &AppType,
    previous: Option<&Provider>,
    generated: &Provider,
    next: &mut Provider,
) -> Result<(), AppError> {
    merge_managed_refresh(app, previous, generated, next)?;
    let expected = previous
        .map(crate::database::Database::provider_update_digest)
        .transpose()?;
    let session = state.db.secret_session();
    let vault = session.read()?;
    state.db.save_provider_with_vault(
        app.as_str(),
        next,
        expected.as_deref(),
        false,
        session,
        &vault,
    )
}

pub(crate) fn read(
    state: &crate::store::AppState,
    app: &AppType,
    provider_id: &str,
) -> Result<Value, AppError> {
    if *app != AppType::Codex {
        return Err(invalid("unsupportedApplication"));
    }
    let provider = state
        .db
        .get_provider_by_id(provider_id, app.as_str())?
        .ok_or_else(|| invalid("providerMissing"))?;
    let overrides = owned(&provider);
    let identity = source_identity(app, &provider)?;
    let source_matches = overrides
        .source_identity
        .as_ref()
        .map_or(!has_explicit_overrides(&overrides), |old| old == &identity);
    let selected = crate::relay::provider_config::selected_model(app, &provider.settings_config);
    let mut names = crate::relay::model_catalog::available_models(&provider);
    if let Some(selected) = &selected {
        if !names.contains(selected) {
            names.push(selected.clone());
        }
    }
    if let Some(rows) = provider
        .settings_config
        .pointer("/modelCatalog/models")
        .and_then(Value::as_array)
    {
        for model in rows
            .iter()
            .filter_map(|r| r.get("model").and_then(Value::as_str))
        {
            if !names.iter().any(|name| name == model) {
                names.push(model.into());
            }
        }
    }
    let models = names.iter().map(|model| {
        let capability = if *app == AppType::Codex { crate::codex_config::resolve_provider_model_capability(&provider, model) } else { Default::default() };
        let mut value = json!({"model":model,"capabilities":capability,
            "declarationOverride":overrides.declaration_models.contains(model),
            "canRestoreDeclaration":source_matches && overrides.declaration_models.contains(model)});
        if let Ok(Some(declaration)) = declaration(&provider, model) { value["declaration"] = json!(declaration); }
        value
    }).collect::<Vec<_>>();
    let intent = if *app == AppType::Codex {
        overrides
            .reasoning
            .clone()
            .unwrap_or(legacy_intent(&provider)?)
    } else {
        ReasoningIntent::ServiceDefault
    };
    let current = selected
        .as_ref()
        .map(|m| crate::codex_config::resolve_provider_model_capability(&provider, m));
    let mut conflicts = current
        .as_ref()
        .map(|c| c.conflicts.clone())
        .unwrap_or_default();
    if !source_matches {
        conflicts.push("sourceChanged".into());
    }
    if let Some(capability) = current {
        if let Err(reason) = capability.validate_intent(&intent) {
            conflicts.push(reason);
        }
    }
    let restore = overrides
        .inherited
        .as_ref()
        .is_some_and(|v| v.source_identity == identity && !v.observed_revision.is_empty());
    let mut value = json!({"app":app.as_str(),"providerId":provider_id,
        "revision":crate::database::Database::provider_update_digest(&provider)?,
        "models":models,"selectedModel":selected,"reasoningIntent":intent,
        "overrides":{"model":overrides.model.is_some(),"reasoning":overrides.reasoning.is_some(),"transport":overrides.transport},
        "templateOptions":crate::codex_reasoning::reasoning_templates().into_iter().filter(|template| crate::codex_config::validate_reasoning_template_for_provider(&provider,template.id).is_ok()).collect::<Vec<_>>(),
        "selectedTemplateId":provider.meta.as_ref().and_then(|m|m.codex_chat_reasoning.as_ref()).and_then(crate::codex_reasoning::reasoning_template_id),
        "canRestoreTransport":source_matches && overrides.transport,
        "canRestoreModel":restore,"canRestoreReasoning":restore,
        "sourceChanged":!source_matches,
        "sourceChangeEffects":{"declarationModels":overrides.declaration_models,"transport":overrides.transport},
        "editable":*app==AppType::Codex,"conflicts":conflicts,"restartRequired":true,
        "manualConfig":false});
    if !restore {
        value["restoreUnavailableReason"] = json!("No current source-bound managed default is available. Refresh this account before restoring automatic model or thinking settings.");
    }
    value["originalSave"] =
        super::edit::model_settings_original_save(state, app, provider_id)?.unwrap_or(Value::Null);
    Ok(value)
}

pub(crate) fn preview(
    state: &crate::store::AppState,
    app: &AppType,
    provider_id: &str,
    patch: &ModelSettingsPatch,
    request_id: &str,
) -> Result<Value, AppError> {
    let previous = state
        .db
        .get_provider_by_id(provider_id, app.as_str())?
        .ok_or_else(|| invalid("providerMissing"))?;
    let draft = apply_patch(app, &previous, patch)?;
    let expected = crate::database::Database::provider_update_digest(&previous)?;
    super::edit::preview_model_settings(state, app, draft, provider_id, request_id, &expected)
}

pub(crate) fn confirm(
    state: &crate::store::AppState,
    app: &AppType,
    provider_id: &str,
    patch: &ModelSettingsPatch,
    request: crate::mode::state::SaveRequest,
) -> Result<Value, AppError> {
    if request.provider_id.as_deref() != Some(provider_id)
        || request.scope != crate::mode::state::SaveScope::ProviderModelSettings
    {
        return Err(invalid("requestMismatch"));
    }
    // Query the authenticated original request before reconstructing a restore.
    // A lost reply must not turn a successful restore into a second operation.
    let previous_result = super::edit::query(state, app, request.clone())?;
    if !matches!(
        previous_result.get("status").and_then(Value::as_str),
        Some("notRecorded" | "unknown")
    ) {
        return Ok(previous_result);
    }
    let previous = state
        .db
        .get_provider_by_id(provider_id, app.as_str())?
        .ok_or_else(|| invalid("providerMissing"))?;
    let draft = apply_patch(app, &previous, patch)?;
    let expected = crate::database::Database::provider_update_digest(&previous)?;
    super::edit::confirm_model_settings(state, app, draft, provider_id, request, &expected)
}

pub(crate) fn public_error(error: AppError) -> String {
    match error {
        AppError::InvalidInput(code) if code.starts_with("provider.modelSettings.") => code,
        other => format!("provider.modelSettings.{}", super::edit::code(&other)),
    }
}

#[cfg(test)]
#[path = "model_settings_tests.rs"]
mod tests;
