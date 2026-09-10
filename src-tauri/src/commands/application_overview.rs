//! Safe application configuration summaries built from existing presentation owners.
use crate::{app_config::AppType, services::provider::ProviderPresentation, store::AppState};
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationOverview {
    pub configurations: Vec<ApplicationConfiguration>,
    pub recent_provider_ids: Vec<String>,
    pub is_additive: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationConfiguration {
    pub provider_id: String,
    pub name: String,
    pub source: ConfigurationSource,
    pub account: Option<AccountReference>,
    pub service_name: Option<String>,
    pub account_label: Option<String>,
    pub configuration_name: Option<String>,
    pub model: Option<String>,
    pub presentation: ProviderPresentation,
    pub selection: ConfigurationSelection,
    pub can_select: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationSource {
    Official,
    Relay,
    Custom,
}
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AccountReference {
    Relay { id: i64 },
    Vendor { id: i64 },
}
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConfigurationSelection {
    Relay,
    Vendor { row_id: i64, plan_id: String },
    Provider,
}

struct AccountConfiguration {
    account: AccountReference,
    service_name: String,
    account_label: String,
    configuration_name: String,
    selection: ConfigurationSelection,
    can_select: bool,
}
fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[tauri::command]
pub async fn get_application_overview(
    state: State<'_, AppState>,
    app: String,
) -> Result<ApplicationOverview, String> {
    let app_type: AppType = app
        .parse()
        .map_err(|e: crate::error::AppError| e.to_string())?;
    let providers = super::provider::get_providers(state.clone(), app.clone())?;
    let relays = super::relay::relay_list_relays(state.clone(), app.clone())?;
    let vendors = super::vendor::vendor_list_accounts(state.clone(), app).await?;
    let mut owners = HashMap::new();
    let mut ambiguous = std::collections::HashSet::new();
    for row in relays {
        for tier in row.tiers {
            if owners.contains_key(&tier.provider_id) {
                ambiguous.insert(tier.provider_id.clone());
            }
            owners.insert(
                tier.provider_id,
                AccountConfiguration {
                    account: AccountReference::Relay { id: row.id },
                    service_name: row.site_name.clone(),
                    account_label: row.account_label.clone(),
                    configuration_name: tier.group_name,
                    selection: ConfigurationSelection::Relay,
                    can_select: true,
                },
            );
        }
    }
    // Legacy tiers can be displayed on multiple account rows. Do not guess an owner.
    for id in ambiguous {
        owners.remove(&id);
    }
    for row in vendors.accounts {
        for plan in row.plans {
            owners.insert(
                plan.provider_id,
                AccountConfiguration {
                    account: AccountReference::Vendor { id: row.id },
                    service_name: row.vendor_name.clone(),
                    account_label: row.account_label.clone(),
                    configuration_name: plan.plan_name,
                    selection: ConfigurationSelection::Vendor {
                        row_id: row.id,
                        plan_id: plan.plan_id,
                    },
                    can_select: plan.can_switch,
                },
            );
        }
    }
    let configurations = providers
        .into_values()
        .map(|view| {
            let owner = owners.remove(&view.provider.id);
            configuration_for_provider(&app_type, view, owner)
        })
        .collect();
    let recent_provider_ids =
        crate::services::application_overview::recent_provider_ids(&state.db, &app_type)
            .map_err(|e| e.to_string())?;
    Ok(ApplicationOverview {
        configurations,
        recent_provider_ids,
        is_additive: app_type.is_additive_mode(),
    })
}

fn configuration_for_provider(
    app_type: &AppType,
    view: super::provider::ProviderView,
    owner: Option<AccountConfiguration>,
) -> ApplicationConfiguration {
    let provider = view.provider;
    let source = match owner.as_ref().map(|o| &o.account) {
        Some(AccountReference::Relay { .. }) => ConfigurationSource::Relay,
        Some(AccountReference::Vendor { .. }) => ConfigurationSource::Official,
        None if view.presentation.is_official
            || crate::relay::managed::is_managed_vendor(&provider.id) =>
        {
            ConfigurationSource::Official
        }
        None if view.presentation.is_managed => ConfigurationSource::Relay,
        None => ConfigurationSource::Custom,
    };
    let can_select = view.presentation.switch_blocked_reason.is_none()
        && owner
            .as_ref()
            .map(|o| o.can_select)
            .unwrap_or(!view.presentation.is_managed);
    let model = crate::relay::provision::selected_model(app_type, &provider.settings_config)
        .and_then(nonempty);
    let (account, service_name, account_label, configuration_name, selection) = match owner {
        Some(owner) => (
            Some(owner.account),
            nonempty(owner.service_name),
            nonempty(owner.account_label),
            nonempty(owner.configuration_name),
            owner.selection,
        ),
        None => (None, None, None, None, ConfigurationSelection::Provider),
    };
    ApplicationConfiguration {
        provider_id: provider.id,
        name: provider.name,
        source,
        account,
        service_name,
        account_label,
        configuration_name,
        model,
        presentation: view.presentation,
        selection,
        can_select,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database::Database,
        provider::Provider,
        services::provider::{provider_presentation_context, provider_presentation_with_context},
    };
    use std::sync::Arc;

    #[test]
    fn overview_has_only_display_fields_and_uses_existing_presentation() {
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        let provider = Provider::with_id(
            "sample".into(),
            "Example configuration".into(),
            serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"secret-token","ANTHROPIC_MODEL":"example-model"}}),
            Some("https://private.example".into()),
        );
        let context = provider_presentation_context(&state, &AppType::Claude);
        let presentation =
            provider_presentation_with_context(&context, &AppType::Claude, &provider);
        let expected = presentation.clone();
        let configuration = configuration_for_provider(
            &AppType::Claude,
            super::super::provider::ProviderView {
                provider,
                presentation,
            },
            Some(AccountConfiguration {
                account: AccountReference::Vendor { id: 7 },
                service_name: "Example vendor".into(),
                account_label: "Work".into(),
                configuration_name: "Standard".into(),
                selection: ConfigurationSelection::Vendor {
                    row_id: 7,
                    plan_id: "standard".into(),
                },
                can_select: true,
            }),
        );
        assert_eq!(configuration.presentation, expected);
        let value = serde_json::to_value(configuration).unwrap();
        assert_eq!(value["source"], "official");
        assert_eq!(value["model"], "example-model");
        assert_eq!(
            value["selection"],
            serde_json::json!({"kind":"vendor","rowId":7,"planId":"standard"})
        );
        assert_eq!(
            value["account"],
            serde_json::json!({"kind":"vendor","id":7})
        );
        assert_eq!(value.as_object().unwrap().len(), 11);
        let serialized = value.to_string();
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("private.example"));
        assert!(!serialized.contains("settingsConfig"));
        let ts = include_str!("../../../src/lib/api/applicationOverview.ts");
        for key in value.as_object().unwrap().keys() {
            assert!(
                ts.contains(&format!("{key}:")),
                "TypeScript field missing: {key}"
            );
        }
        assert!(ts.contains("\"get_application_overview\""));
    }

    #[test]
    fn failed_selection_and_provisioning_do_not_create_history() {
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        let id = crate::relay::provision::provider_id_for("https://relay.example", Some(1), 2);
        let provider = Provider::with_id(
            id.clone(),
            "Example tier".into(),
            serde_json::json!({}),
            None,
        );
        state.db.save_provider("codex", &provider).unwrap();
        assert!(
            super::super::provider::switch_provider_test_hook(&state, AppType::Codex, &id).is_err()
        );
        assert!(crate::services::application_overview::recent_provider_ids(
            &state.db,
            &AppType::Codex
        )
        .unwrap()
        .is_empty());
        let context = provider_presentation_context(&state, &AppType::Codex);
        let presentation = provider_presentation_with_context(&context, &AppType::Codex, &provider);
        let entry = configuration_for_provider(
            &AppType::Codex,
            super::super::provider::ProviderView {
                provider,
                presentation,
            },
            None,
        );
        assert!(!entry.can_select);
        assert!(entry.account.is_none());
    }
    #[test]
    #[serial_test::serial]
    fn pi_presentation_reads_enabled_membership_and_default_from_native_state() {
        let _agent = crate::pi_config::test_support::TestAgentDir::new();
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        let models = crate::pi_config::get_pi_models_path().unwrap();
        std::fs::create_dir_all(models.parent().unwrap()).unwrap();
        std::fs::write(models, r#"{"providers":{"enabled":{}}}"#).unwrap();
        std::fs::write(
            crate::pi_config::get_pi_settings_path().unwrap(),
            r#"{"defaultProvider":"enabled","defaultModel":"example-model"}"#,
        )
        .unwrap();
        let context = provider_presentation_context(&state, &AppType::Pi);
        for (id, expected) in [("enabled", true), ("disabled", false)] {
            let provider = Provider::with_id(id.into(), id.into(), serde_json::json!({}), None);
            let presentation =
                provider_presentation_with_context(&context, &AppType::Pi, &provider);
            assert_eq!(presentation.is_in_config, expected);
            assert_eq!(presentation.is_default_model, expected);
        }
        std::fs::write(
            crate::pi_config::get_pi_models_path().unwrap(),
            "invalid json",
        )
        .unwrap();
        let context = provider_presentation_context(&state, &AppType::Pi);
        let provider = Provider::with_id(
            "enabled".into(),
            "Enabled".into(),
            serde_json::json!({}),
            None,
        );
        let presentation = provider_presentation_with_context(&context, &AppType::Pi, &provider);
        assert!(!presentation.is_in_config);
        assert!(!presentation.is_default_model);
    }
}
