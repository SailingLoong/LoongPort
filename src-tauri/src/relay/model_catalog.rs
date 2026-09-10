//! Remote model availability is a relay-owned snapshot, separate from editable routing mappings.
use super::{managed, provision};
use crate::{app_config::AppType, database::Database, error::AppError, provider::Provider};

/// Vendor catalogs are authored by the vendor domain, rather than discovered from relay keys.
/// Custom providers retain their existing editable-catalog behavior.
pub(crate) fn available_models(provider: &Provider) -> Vec<String> {
    if managed::is_managed(&provider.id) && !managed::is_managed_vendor(&provider.id) {
        provider.available_models.clone().unwrap_or_default()
    } else {
        provision::models_from_settings(&provider.settings_config)
    }
}

/// The same platform filtering is used by configuration generation and availability snapshots.
pub(crate) fn filter_models(app: &AppType, models: &[String]) -> Vec<String> {
    models
        .iter()
        .filter(|model| {
            if matches!(app, AppType::Gemini) {
                model.to_ascii_lowercase().starts_with("gemini-")
            } else {
                !provision::is_image_model(model)
            }
        })
        .cloned()
        .collect()
}

/// Validate against remote availability, then modify only the user's explicit model selection.
/// A selected Codex model needs a catalog row for native clients; existing overrides stay intact.
pub(crate) fn select_model(
    app: &AppType,
    provider: &Provider,
    model: &str,
) -> Result<serde_json::Value, AppError> {
    let model = model.trim();
    if model.is_empty()
        || !available_models(provider)
            .iter()
            .any(|candidate| candidate == model)
    {
        return Err(AppError::localized(
            "model.unavailable",
            "所选模型不在此配置的可用模型列表中。",
            "The selected model is not available for this configuration.",
        ));
    }
    let mut settings = provider.settings_config.clone();
    match app {
        AppType::Codex => {
            if !provision::models_from_settings(&settings)
                .iter()
                .any(|candidate| candidate == model)
            {
                let object = settings.as_object_mut().ok_or_else(|| {
                    AppError::localized(
                        "model.invalid_configuration",
                        "无法读取模型配置，请检查配置内容。",
                        "Could not read the model configuration. Please check its contents.",
                    )
                })?;
                let catalog = object
                    .entry("modelCatalog")
                    .or_insert_with(|| serde_json::json!({"models":[]}));
                let rows = catalog
                    .as_object_mut()
                    .ok_or_else(|| {
                        AppError::localized(
                            "model.invalid_configuration",
                            "无法读取模型配置，请检查配置内容。",
                            "Could not read the model configuration. Please check its contents.",
                        )
                    })?
                    .entry("models")
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .ok_or_else(|| {
                        AppError::localized(
                            "model.invalid_configuration",
                            "无法读取模型配置，请检查配置内容。",
                            "Could not read the model configuration. Please check its contents.",
                        )
                    })?;
                rows.push(serde_json::json!({"model": model}));
            }
            let config = settings
                .get("config")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AppError::localized(
                        "model.invalid_configuration",
                        "无法读取模型配置，请检查配置内容。",
                        "Could not read the model configuration. Please check its contents.",
                    )
                })?;
            settings["config"] =
                crate::codex_config::update_codex_toml_field(config, "model", model)
                    .map_err(AppError::Config)?
                    .into();
            Ok(settings)
        }
        AppType::GrokBuild => {
            let config = settings
                .get("config")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    AppError::localized(
                        "model.invalid_configuration",
                        "无法读取模型配置，请检查配置内容。",
                        "Could not read the model configuration. Please check its contents.",
                    )
                })?;
            settings["config"] =
                crate::grok_config::update_selected_model_string(config, "model", model)?.into();
            Ok(settings)
        }
        _ => provision::select_env_model(app, &settings, model),
    }
}

/// Existing unedited managed mappings were generated from upstream inventories.
/// Seed those snapshots during schema migration, without treating user mappings as remote facts.
pub(crate) fn seed_legacy_inventories(conn: &rusqlite::Connection) -> Result<(), AppError> {
    let mut statement = conn.prepare("SELECT id, app_type, settings_config FROM providers WHERE available_models IS NULL AND user_edited=0")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, app, settings) in rows {
        if !managed::is_managed(&id) || managed::is_managed_vendor(&id) {
            continue;
        }
        let Ok(app_type) = app.parse::<AppType>() else {
            continue;
        };
        if !provision::supports_model_catalog(&app_type) {
            continue;
        }
        let Ok(settings) = serde_json::from_str(&settings) else {
            continue;
        };
        let models = filter_models(&app_type, &provision::models_from_settings(&settings));
        if models.is_empty() {
            continue;
        }
        conn.execute("UPDATE providers SET available_models=?1 WHERE app_type=?2 AND id=?3 AND available_models IS NULL",
            rusqlite::params![serde_json::to_string(&models).map_err(|error| AppError::Database(error.to_string()))?, app, id])?;
    }
    Ok(())
}

/// A startup repair performs only model-list GETs with existing provider keys. It never
/// provisions accounts, creates keys, changes live configuration, or writes selection history.
pub(crate) async fn repair_missing(db: &Database) -> Result<Vec<(AppType, String)>, AppError> {
    repair_missing_with(db, |origin, key| async move {
        super::sub2api::list_models(&origin, &key).await
    })
    .await
}

async fn repair_missing_with<F, Fut>(
    db: &Database,
    fetch: F,
) -> Result<Vec<(AppType, String)>, AppError>
where
    F: Fn(String, String) -> Fut,
    Fut: std::future::Future<Output = Result<Option<Vec<String>>, AppError>>,
{
    use futures::StreamExt;
    let relays = {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Database(error.to_string()))?;
        super::creds::list(&conn)?
    };
    let mut targets = Vec::new();
    for app in provision::model_catalog_apps() {
        for provider in db.get_all_providers(app.as_str())?.into_values() {
            if !managed::is_managed(&provider.id)
                || managed::is_managed_vendor(&provider.id)
                || provider.available_models.is_some()
            {
                continue;
            }
            let mut owners = relays.iter().filter(|relay| {
                managed::belongs_to_relay(&provider, &relay.site_origin, relay.account_id)
            });
            let Some(owner) = owners.next() else { continue };
            if owners.next().is_some() {
                continue;
            }
            let Some(adapter) = crate::proxy::providers::get_adapter(app) else {
                continue;
            };
            let Ok(endpoint) = adapter.extract_base_url(&provider) else {
                continue;
            };
            let expected_endpoint =
                super::sub2api::base_url_for(app, &owner.site_origin, &owner.api_base_url);
            let (Ok(expected_url), Ok(endpoint_url)) = (
                url::Url::parse(&expected_endpoint),
                url::Url::parse(&endpoint),
            ) else {
                continue;
            };
            if expected_url.origin() != endpoint_url.origin() {
                continue;
            }
            let Some(key) = provision::extract_api_key(&provider.settings_config, app)
                .filter(|key| !key.trim().is_empty())
            else {
                continue;
            };
            targets.push((app.clone(), provider, owner.clone(), key));
        }
    }
    let mut changed = Vec::new();
    let mut requests =
        futures::stream::iter(targets.into_iter().map(|(app, provider, owner, key)| {
            let request = fetch(owner.site_origin.clone(), key);
            async move {
                let result =
                    tokio::time::timeout(std::time::Duration::from_secs(10), request).await;
                (app, provider, owner, result)
            }
        }))
        .buffer_unordered(2);
    while let Some((app, provider, owner, result)) = requests.next().await {
        if let Ok(Ok(Some(models))) = result {
            let models = filter_models(&app, &provision::normalize_model_names(models));
            if db.fill_missing_available_models(app.as_str(), &provider, &owner, &models)? {
                changed.push((app, provider.id));
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn relay() -> Provider {
        Provider::with_id(
            provision::provider_id_for("https://relay.example", Some(1), 2),
            "Example".into(),
            json!({"auth":{"OPENAI_API_KEY":"example-key"},"config":"model = \"custom\"\nmodel_reasoning_effort = \"high\"\nmodel_provider = \"relay\"\n[model_providers.relay]\nbase_url = \"https://relay.example/v1\"\n", "modelCatalog":{"models":[{"model":"custom", "contextWindow":100000}]}}),
            Some("https://relay.example".into()),
        )
    }

    #[tokio::test]
    async fn repair_preserves_user_configuration_and_skips_stale_network_results() {
        let db = Database::memory().unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO loongport_relay(site_origin,account_id) VALUES('https://relay.example',1)", []).unwrap();
        let provider = relay();
        db.save_provider("codex", &provider).unwrap();
        db.set_user_edited("codex", &provider.id, true).unwrap();
        let changed =
            repair_missing_with(&db, |_, _| async { Ok(Some(vec!["remote-model".into()])) })
                .await
                .unwrap();
        assert_eq!(changed, vec![(AppType::Codex, provider.id.clone())]);
        let after = db
            .get_provider_by_id(&provider.id, "codex")
            .unwrap()
            .unwrap();
        assert_eq!(after.settings_config, provider.settings_config);
        assert_eq!(available_models(&after), vec!["remote-model"]);
        assert!(db.get_user_edited("codex", &provider.id).unwrap());
        assert!(db
            .get_setting("application_recent_providers_codex")
            .unwrap()
            .is_none());
        // A stale generic edit payload cannot erase the separate snapshot.
        db.save_provider("codex", &provider).unwrap();
        assert_eq!(
            db.get_provider_by_id(&provider.id, "codex")
                .unwrap()
                .unwrap()
                .available_models,
            after.available_models
        );
        db.delete_provider("codex", &provider.id).unwrap();
        db.save_provider("codex", &provider).unwrap();
        let changed = repair_missing_with(&db, |_, _| {
            let mut edited = provider.clone();
            edited.settings_config["auth"]["OPENAI_API_KEY"] = json!("replaced-key");
            db.save_provider("codex", &edited).unwrap();
            async { Ok(Some(vec!["stale-model".into()])) }
        })
        .await
        .unwrap();
        assert!(changed.is_empty());
        assert!(db
            .get_provider_by_id(&provider.id, "codex")
            .unwrap()
            .unwrap()
            .available_models
            .is_none());
    }

    #[tokio::test]
    async fn failed_repair_leaves_missing_and_image_only_inventory_is_authoritative() {
        let db = Database::memory().unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO loongport_relay(site_origin,account_id) VALUES('https://relay.example',1)", []).unwrap();
        let provider = relay();
        db.save_provider("codex", &provider).unwrap();
        let before = db.conn.lock().unwrap().total_changes();
        assert!(repair_missing_with(&db, |_, _| async { Ok(None) })
            .await
            .unwrap()
            .is_empty());
        assert_eq!(db.conn.lock().unwrap().total_changes(), before);
        repair_missing_with(&db, |_, _| async { Ok(Some(vec!["gpt-image-2".into()])) })
            .await
            .unwrap();
        let after = db
            .get_provider_by_id(&provider.id, "codex")
            .unwrap()
            .unwrap();
        assert_eq!(after.available_models, Some(Vec::new()));
        assert!(available_models(&after).is_empty());
        assert!(repair_missing_with(&db, |_, _| async {
            panic!("known inventories are not fetched")
        })
        .await
        .unwrap()
        .is_empty());
    }

    #[tokio::test]
    async fn repair_never_sends_a_custom_endpoint_key_to_the_original_site() {
        let db = Database::memory().unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO loongport_relay(site_origin,account_id) VALUES('https://relay.example',1)", []).unwrap();
        let mut provider = relay();
        provider.settings_config["config"] = json!("model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://other.example/v1\"\n");
        db.save_provider("codex", &provider).unwrap();
        let before = db.conn.lock().unwrap().total_changes();
        let changed = repair_missing_with(&db, |_, _| async { panic!("must not send this key") })
            .await
            .unwrap();
        assert!(changed.is_empty());
        assert_eq!(db.conn.lock().unwrap().total_changes(), before);
    }

    #[tokio::test]
    async fn repair_supports_recorded_split_origin_and_rejects_changed_binding() {
        let db = Database::memory().unwrap();
        db.conn.lock().unwrap().execute("INSERT INTO loongport_relay(site_origin,account_id,api_base_url) VALUES('https://relay.example',1,'https://api.example')", []).unwrap();
        let mut provider = relay();
        provider.settings_config["config"] = json!("model_provider = \"relay\"\n[model_providers.relay]\nbase_url = \"https://api.example/v1\"\n");
        db.save_provider("codex", &provider).unwrap();
        let changed = repair_missing_with(&db, |origin, _| async move {
            assert_eq!(origin, "https://relay.example");
            Ok(Some(vec!["remote-model".into()]))
        })
        .await
        .unwrap();
        assert_eq!(changed.len(), 1);
        db.delete_provider("codex", &provider.id).unwrap();
        db.save_provider("codex", &provider).unwrap();
        let changed = repair_missing_with(&db, |_, _| {
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE loongport_relay SET api_base_url='https://new.example'",
                    [],
                )
                .unwrap();
            async { Ok(Some(vec!["stale-model".into()])) }
        })
        .await
        .unwrap();
        assert!(changed.is_empty());
        assert!(db
            .get_provider_by_id(&provider.id, "codex")
            .unwrap()
            .unwrap()
            .available_models
            .is_none());
    }

    #[test]
    fn available_inventory_is_independent_of_editable_mapping() {
        let mut provider = relay();
        assert!(available_models(&provider).is_empty());
        provider.available_models = Some(vec!["remote-model".into()]);
        assert_eq!(available_models(&provider), vec!["remote-model"]);
        assert_eq!(
            crate::proxy::auto_strategy::tier_models(&provider),
            vec!["remote-model"]
        );
        assert!(serde_json::to_value(&provider)
            .unwrap()
            .get("available_models")
            .is_none());
        assert_eq!(
            filter_models(
                &AppType::Gemini,
                &["gpt-text".into(), "gemini-pro".into(), "gpt-image-2".into()]
            ),
            vec!["gemini-pro"]
        );
    }

    #[test]
    fn explicit_model_selection_adds_only_selected_row_and_retains_overrides() {
        let mut provider = relay();
        provider.available_models = Some(vec!["remote-model".into(), "custom".into()]);
        let selected = select_model(&AppType::Codex, &provider, "remote-model").unwrap();
        assert_eq!(
            selected["modelCatalog"]["models"][0],
            provider.settings_config["modelCatalog"]["models"][0]
        );
        assert_eq!(
            selected["modelCatalog"]["models"][1],
            json!({"model":"remote-model"})
        );
        assert_eq!(
            provision::selected_model(&AppType::Codex, &selected).as_deref(),
            Some("remote-model")
        );
        assert!(select_model(&AppType::Codex, &provider, "unknown").is_err());
    }
}
