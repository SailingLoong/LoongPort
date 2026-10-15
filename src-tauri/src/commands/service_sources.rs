//! Read existing managed sources without provisioning, probing or writing state.
use crate::{
    app_config::AppType,
    provider::Provider,
    relay::managed,
    services::service_sources::{
        project_service_sources, relay_account_identities, source_usage_state,
        verified_relay_account, ServiceSource, ServiceSourceBinding, ServiceSourceCandidate,
        SourceAccountReference,
    },
    store::AppState,
};

#[tauri::command]
pub async fn get_service_sources(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ServiceSource>, String> {
    get_service_sources_impl(state.inner())
}

fn get_service_sources_impl(state: &AppState) -> Result<Vec<ServiceSource>, String> {
    let relay_accounts = {
        let conn = state.db.conn.lock().map_err(|error| error.to_string())?;
        relay_account_identities(&conn).map_err(|error| error.to_string())?
    };
    let mut candidates = Vec::new();
    for app in AppType::all() {
        let overview = super::application_overview::application_overview(state, &app)?;
        let providers = state
            .db
            .get_all_providers(app.as_str())
            .map_err(|error| error.to_string())?;
        for configuration in overview.configurations {
            let Some(account) = configuration.account else {
                // Custom or ambiguous legacy records have no verified shared owner.
                continue;
            };
            let owned = match &account {
                SourceAccountReference::Relay { id } => {
                    managed::is_managed(&configuration.provider_id)
                        && !managed::is_managed_vendor(&configuration.provider_id)
                        && providers
                            .get(&configuration.provider_id)
                            .is_some_and(|provider| {
                                verified_relay_provider(
                                    provider,
                                    relay_accounts.get(id).copied().flatten(),
                                )
                            })
                }
                SourceAccountReference::Vendor { .. } => {
                    managed::is_managed_vendor(&configuration.provider_id)
                }
            };
            if !owned {
                continue;
            }
            candidates.push(ServiceSourceCandidate {
                source_id: configuration.provider_id.clone(),
                account,
                name: configuration.name,
                service_name: configuration.service_name,
                account_label: configuration.account_label,
                configuration_name: configuration.configuration_name,
                binding: ServiceSourceBinding {
                    app_id: app.as_str().into(),
                    provider_id: configuration.provider_id,
                    model: configuration.model,
                    usage_state: source_usage_state(
                        app.is_additive_mode(),
                        configuration.presentation.is_current,
                        configuration.presentation.is_in_config,
                    ),
                    can_select: configuration.can_select,
                },
            });
        }
    }
    Ok(project_service_sources(candidates))
}

fn verified_relay_provider(provider: &Provider, account_id: Option<i64>) -> bool {
    verified_relay_account(
        account_id,
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.loongport_account_id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderMeta;

    fn relay_provider(site: &str, account: Option<i64>) -> Provider {
        let mut provider = Provider::with_id(
            managed::provider_id_for(site, account, 3),
            "Standard".into(),
            serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-test-key"}}),
            Some(site.into()),
        );
        provider.meta = Some(ProviderMeta {
            loongport_account_id: account,
            ..Default::default()
        });
        provider
    }

    #[test]
    #[serial_test::serial]
    fn source_collection_does_not_claim_legacy_tier_for_the_only_site_account() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().unwrap(),
        ))
        .unwrap();
        let site = "https://relay.example";
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO loongport_relay(id,site_origin,account_id) VALUES(11,?1,77)",
                [site],
            )
            .unwrap();
        let provider = relay_provider(site, None);
        state.db.save_provider("claude", &provider).unwrap();
        // The legacy App display deliberately retains its site-only association.
        let overview =
            super::super::application_overview::application_overview(&state, &AppType::Claude)
                .unwrap();
        assert!(overview
            .configurations
            .iter()
            .any(|row| row.provider_id == provider.id && row.account.is_some()));
        let before = state.db.conn.lock().unwrap().total_changes();
        assert!(get_service_sources_impl(&state).unwrap().is_empty());
        assert_eq!(state.db.conn.lock().unwrap().total_changes(), before);
    }

    #[test]
    #[serial_test::serial]
    fn source_collection_uses_external_account_identity_and_retains_cross_app_references() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().unwrap(),
        ))
        .unwrap();
        let site = "https://relay.example";
        for (row, account) in [(11, 77), (12, 88)] {
            state
                .db
                .conn
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO loongport_relay(id,site_origin,account_id) VALUES(?1,?2,?3)",
                    rusqlite::params![row, site, account],
                )
                .unwrap();
            let provider = relay_provider(site, Some(account));
            for app in ["claude", "codex"] {
                state.db.save_provider(app, &provider).unwrap();
            }
        }
        let before = state.db.conn.lock().unwrap().total_changes();
        let sources = get_service_sources_impl(&state).unwrap();
        assert_eq!(sources.len(), 2);
        for (row, account) in [(11, 77), (12, 88)] {
            let source = sources
                .iter()
                .find(|source| source.account == SourceAccountReference::Relay { id: row })
                .unwrap();
            assert_eq!(
                source.source_id,
                managed::provider_id_for(site, Some(account), 3)
            );
            assert_eq!(
                source
                    .bindings
                    .iter()
                    .map(|b| b.app_id.as_str())
                    .collect::<Vec<_>>(),
                ["claude", "codex"]
            );
        }
        assert_eq!(state.db.conn.lock().unwrap().total_changes(), before);
        assert!(!serde_json::to_string(&sources)
            .unwrap()
            .contains("synthetic-test-key"));
    }

    #[test]
    #[serial_test::serial]
    fn source_collection_retains_separate_vendor_plans_for_the_same_account() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let state = AppState::new(std::sync::Arc::new(
            crate::database::Database::memory().unwrap(),
        ))
        .unwrap();
        let vendor = crate::vendor::Vendor::OpenCode;
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO loongport_vendor(id,vendor_id,account_id) VALUES(21,?1,'account-a')",
                [vendor.vendor_id()],
            )
            .unwrap();
        for plan in crate::vendor::plans(vendor) {
            let provider = Provider::with_id(
                crate::vendor::provision::provider_id_for(plan.id_segment, "account-a"),
                plan.display_name.into(),
                serde_json::json!({}),
                None,
            );
            state.db.save_provider("codex", &provider).unwrap();
        }
        let sources = get_service_sources_impl(&state).unwrap();
        assert_eq!(sources.len(), crate::vendor::plans(vendor).len());
        assert!(sources.len() > 1);
        assert!(sources
            .iter()
            .all(|source| source.account == SourceAccountReference::Vendor { id: 21 }));
        assert_ne!(sources[0].source_id, sources[1].source_id);
    }

    #[test]
    fn sole_site_account_does_not_claim_legacy_or_mismatched_provider() {
        let mut provider = Provider::with_id(
            managed::provider_id_for("https://relay.example", None, 3),
            "Standard".into(),
            serde_json::json!({}),
            Some("https://relay.example".into()),
        );
        assert!(!verified_relay_provider(&provider, Some(7)));
        provider.meta = Some(ProviderMeta::default());
        assert!(!verified_relay_provider(&provider, Some(7)));
        provider.meta.as_mut().unwrap().loongport_account_id = Some(8);
        assert!(!verified_relay_provider(&provider, Some(7)));
        provider.meta.as_mut().unwrap().loongport_account_id = Some(7);
        assert!(verified_relay_provider(&provider, Some(7)));
        assert!(!verified_relay_provider(&provider, None));
    }
}
