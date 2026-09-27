//! Accept native configuration at the boundary where the user enables routing.
//! A native snapshot is a complete configuration, never a credential update to
//! whichever database provider happened to be selected previously.
use crate::{app_config::AppType, database::Database, error::AppError, provider::Provider};
use serde_json::{json, Value};

fn comparable_settings(settings: &Value) -> Result<Value, AppError> {
    let mut value = settings.clone();
    if let Some(config) = settings.get("config").and_then(Value::as_str) {
        let parsed: toml::Value = toml::from_str(config)
            .map_err(|error| AppError::Config(format!("Invalid Codex configuration: {error}")))?;
        value["config"] =
            serde_json::to_value(parsed).map_err(|error| AppError::Config(error.to_string()))?;
    }
    Ok(value)
}

fn comparable_managed_config(config: &str) -> Result<toml::Value, AppError> {
    let config = crate::codex_config::strip_codex_unified_session_bucket(config)?;
    let mut parsed: toml::Value = toml::from_str(&config)
        .map_err(|error| AppError::Config(format!("Invalid Codex configuration: {error}")))?;
    let generated_catalog = parsed
        .get("model_catalog_json")
        .and_then(toml::Value::as_str)
        .and_then(|path| std::path::Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .is_some_and(crate::codex_config::is_our_model_catalog_filename);
    if generated_catalog {
        if let Some(table) = parsed.as_table_mut() {
            table.remove("model_catalog_json");
        }
    }
    Ok(parsed)
}

impl super::ProviderService {
    /// Caller holds the Codex switch lock and has verified native files are not
    /// proxy placeholders. Only complete equivalent configurations are reused.
    pub(crate) fn adopt_codex_native_configuration(
        db: &Database,
        native: Value,
    ) -> Result<Provider, AppError> {
        if let Some(current_id) = db.get_current_provider("codex")? {
            if let Some(current) = db.get_provider_by_id(&current_id, "codex")? {
                if let Some(account_id) = current
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.managed_account_id_for("codex_oauth"))
                {
                    let owned_auth = native.get("auth").is_some_and(|auth| {
                        crate::codex_config::codex_auth_matches_recorded_managed_oauth(
                            auth,
                            &account_id,
                        )
                        .unwrap_or(false)
                    });
                    let effective = super::live::build_effective_settings_with_common_config(
                        db,
                        &AppType::Codex,
                        &current,
                    )?;
                    let same_config = native
                        .get("config")
                        .and_then(Value::as_str)
                        .zip(effective.get("config").and_then(Value::as_str))
                        .is_some_and(|(native, expected)| {
                            comparable_managed_config(native).ok()
                                == comparable_managed_config(expected).ok()
                        });
                    if owned_auth && same_config {
                        return Ok(current);
                    }
                }
            }
        }
        let mut candidate = Provider::with_id(
            uuid::Uuid::new_v4().to_string(),
            "Codex native configuration".into(),
            native,
            None,
        );
        let config = candidate
            .settings_config
            .get("config")
            .and_then(Value::as_str);
        let key = crate::codex_config::extract_codex_api_key(
            candidate.settings_config.get("auth"),
            config,
        );
        let official = key.is_none()
            && candidate
                .settings_config
                .get("auth")
                .is_some_and(crate::codex_config::codex_auth_has_login_material);
        candidate.category = Some(if official { "official" } else { "custom" }.into());
        // ChatGPT auth remains native-owned; a third-party snapshot stores only
        // its own effective API credential, never an unrelated native login.
        candidate.settings_config["auth"] = key
            .map(|key| json!({"OPENAI_API_KEY":key}))
            .unwrap_or_else(|| json!({}));
        candidate.meta = Some(crate::provider::ProviderMeta {
            common_config_enabled: Some(false),
            ..Default::default()
        });
        Self::validate_provider_settings(&AppType::Codex, &candidate)?;
        Self::validate_proxy_takeover_target(&AppType::Codex, &candidate)?;
        let snapshot = comparable_settings(&candidate.settings_config)?;
        let profile = crate::proxy::providers::resolve_codex_catalog_tool_profile(&candidate);
        let mut selected = None;
        for existing in db.get_all_providers("codex")?.into_values() {
            if crate::relay::is_managed(&existing.id) || existing.uses_managed_account_auth() {
                continue;
            }
            let Ok(effective) = super::live::build_effective_settings_with_common_config(
                db,
                &AppType::Codex,
                &existing,
            ) else {
                continue;
            };
            if crate::proxy::providers::resolve_codex_catalog_tool_profile(&existing) == profile
                && comparable_settings(&effective).ok().as_ref() == Some(&snapshot)
            {
                selected = Some(existing);
                break;
            }
        }
        let selected = selected.unwrap_or(candidate);
        super::validate_provider_selection(db, &AppType::Codex, &selected.id)?;
        if db.get_provider_by_id(&selected.id, "codex")?.is_none() {
            db.save_provider("codex", &selected)?;
            crate::proxy::application_routing::note_provider_created(db, "codex", &selected.id)?;
        }
        db.set_current_provider("codex", &selected.id)?;
        crate::settings::set_current_provider(&AppType::Codex, Some(&selected.id))?;
        crate::proxy::auto_strategy::set_model_pref(db, "codex", None)?;
        Ok(selected)
    }
}
