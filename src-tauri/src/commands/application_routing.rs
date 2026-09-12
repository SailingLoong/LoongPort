use super::auto_mode::{tier_board_impl, TierBoardModelOption, TierBoardTier};
use crate::{app_config::AppType, proxy::application_routing as routing, store::AppState};
use std::str::FromStr;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRoutingTier {
    #[serde(flatten)]
    pub tier: TierBoardTier,
    pub skip_reason: Option<String>,
    pub error_rate: Option<f64>,
    pub can_failover: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRouting {
    pub auto_failover_enabled: bool,
    pub routing_active: bool,
    pub model: Option<String>,
    pub model_options: Vec<TierBoardModelOption>,
    pub tiers: Vec<ApplicationRoutingTier>,
}

#[tauri::command]
pub async fn get_application_routing(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<ApplicationRouting, String> {
    application_routing_impl(&state, &app_type).await
}

pub(crate) async fn application_routing_impl(
    state: &AppState,
    app_type: &str,
) -> Result<ApplicationRouting, String> {
    let supports_proxy = AppType::from_str(app_type)
        .map_err(|e| e.to_string())?
        .supports_local_proxy();
    let board = tier_board_impl(state, app_type).await?;
    let providers = state
        .db
        .get_all_providers(app_type)
        .map_err(|e| e.to_string())?;
    let enabled = supports_proxy
        && routing::failover_enabled(&state.db, app_type).map_err(|e| e.to_string())?;
    let model_routing_active = supports_proxy
        && routing::takeover_enabled(&state.db, app_type).map_err(|e| e.to_string())?
        && state.proxy_service.is_running().await;
    let current_position = board.tiers.iter().position(|p| p.is_current);
    let current_official_account = current_position
        .and_then(|index| providers.get(&board.tiers[index].provider_id))
        .is_some_and(|p| {
            crate::proxy::provider_router::provider_supports_proxy_routing(app_type, p)
                && !crate::proxy::provider_router::provider_supports_failover(app_type, p)
        });
    let stats = state
        .db
        .provider_attempt_error_rates(app_type)
        .unwrap_or_default();
    let tiers = board
        .tiers
        .into_iter()
        .map(|tier| {
            let exclusion = providers
                .get(&tier.provider_id)
                .and_then(|p| routing::fallback_exclusion(&state.db, app_type, p));
            let circuit_open = tier.breaker_state.as_deref() == Some("open")
                && tier.breaker_reopen_in_secs.is_some_and(|s| s > 0);
            let skip_reason = if !supports_proxy
                || (tier.is_current && exclusion != Some("native_configuration"))
            {
                None
            } else if let Some(reason) = exclusion {
                Some(reason.to_string())
            } else if enabled && current_official_account {
                Some("current_official_account".to_string())
            } else if circuit_open {
                Some("circuit_open".to_string())
            } else if enabled && current_position.is_some_and(|index| tier.position < index) {
                Some("before_current".to_string())
            } else {
                None
            };
            let error_rate = stats.get(&tier.provider_id).copied();
            ApplicationRoutingTier {
                can_failover: supports_proxy
                    && providers.get(&tier.provider_id).is_some_and(|p| {
                        crate::proxy::provider_router::provider_supports_failover(app_type, p)
                    }),
                tier,
                skip_reason,
                error_rate,
            }
        })
        .collect();
    Ok(ApplicationRouting {
        auto_failover_enabled: enabled,
        routing_active: model_routing_active,
        model: board.model,
        model_options: if model_routing_active {
            current_model_options(&state.db, app_type, board.model_options)
        } else {
            Vec::new()
        },
        tiers,
    })
}

#[tauri::command]
pub async fn set_application_priority(
    state: tauri::State<'_, AppState>,
    app_type: String,
    ordered_ids: Vec<String>,
) -> Result<(), String> {
    routing::set_order(&state.db, &app_type, &ordered_ids).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_application_failover(
    state: tauri::State<'_, AppState>,
    app_type: String,
    enabled: bool,
) -> Result<(), String> {
    routing::set_failover(&state.db, &app_type, enabled)
        .await
        .map_err(|e| e.to_string())
}

fn current_model_options(
    db: &crate::Database,
    app: &str,
    options: Vec<TierBoardModelOption>,
) -> Vec<TierBoardModelOption> {
    options
        .into_iter()
        .filter(|option| routing::current_supports_model(db, app, &option.model).unwrap_or(false))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    #[serial_test::serial]
    fn advertised_models_match_current_setter_acceptance() {
        let db = crate::Database::memory().unwrap();
        for (id, models) in [
            ("current", vec!["shared", "current-only"]),
            ("other", vec!["shared", "other-only"]),
        ] {
            let provider = crate::provider::Provider::with_id(
                id.into(),
                id.into(),
                serde_json::json!({"modelCatalog":{"models":models.iter().map(|m| serde_json::json!({"model":m})).collect::<Vec<_>>()}}),
                None,
            );
            db.save_provider("claude", &provider).unwrap();
        }
        db.set_current_provider("claude", "current").unwrap();
        let options = ["shared", "current-only", "other-only"]
            .into_iter()
            .map(|model| super::TierBoardModelOption {
                model: model.into(),
                tier_count: 1,
                cheapest_price_per_million: None,
            })
            .collect();
        let advertised = super::current_model_options(&db, "claude", options);
        assert_eq!(
            advertised
                .iter()
                .map(|o| o.model.as_str())
                .collect::<Vec<_>>(),
            vec!["shared", "current-only"]
        );
        for option in advertised {
            super::routing::set_model(&db, "claude", Some(&option.model)).unwrap();
        }
        assert!(super::routing::set_model(&db, "claude", Some("other-only")).is_err());
        assert_eq!(
            super::routing::ordered_providers(&db, "claude")
                .unwrap()
                .len(),
            2
        );
    }
}
