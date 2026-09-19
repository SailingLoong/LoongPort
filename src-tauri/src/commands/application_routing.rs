use super::auto_mode::{tier_board_impl, TierBoardModelOption, TierBoardTier};
use crate::relay::model_verification::target as verification_target;
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
    /// 模型验证资格（与 relay 行级 `TierInfo::can_verify_models` 同名同源）：
    /// app 类型支持验证 **且** 是 LoongPort 托管的中转站档位 —— 工作台表格
    /// 混着官网直连/自定义配置，验证入口只给能验的行。
    pub can_verify_models: bool,
    /// 档位模型目录（provision 嗅探 `/v1/models` 落库，唯源
    /// `auto_strategy::tier_models`——与模型选择器并集同一来源）。工作台
    /// 的模型筛选按它命中「分组支持」，而非只看当前 `effective_model`；
    /// 空目录（非 Codex 系/未嗅探）回落单模型语义。
    pub models: Vec<String>,
    /// 订阅限额的重置窗口（provision 从服务端限额 × key 用量算出落库；
    /// 非订阅档位为空）。工作台「下次重置」列与 tooltip、账号详情的窗口表都读它。
    pub subscription_windows: Vec<crate::relay::tier_windows::SubscriptionWindow>,
    /// 所有窗口里最早的重置时刻（epoch 秒）——「优先消耗即将作废的额度」的排序键；
    /// 窗口都没开始 / 非订阅档位为 `None`（排序时排最后，稳定）。
    pub next_reset_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRouting {
    pub auto_failover_enabled: bool,
    pub routing_active: bool,
    pub model: Option<String>,
    pub model_options: Vec<TierBoardModelOption>,
    /// 故障切换链的原始 id 序（可能含上游已删除的幽灵；前端据此算优先级号与
    /// 「应用此顺序」的待应用差异）。链未初始化时回落全量显示序——与 migrate 播种等价。
    pub chain_ids: Vec<String>,
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
    let app = AppType::from_str(app_type).map_err(|e| e.to_string())?;
    let supports_proxy = app.supports_local_proxy();
    // 验证资格的判据收在 target 模块（relay 行级同一条）；这里只对托管档位放行。
    let verification_supported = verification_target::supports_app_type(&app);
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
    let blocked = routing::blocked_tier_ids(&state.db, app_type);
    let chain = routing::chain_ids(&state.db, app_type).map_err(|e| e.to_string())?;
    let tiers = board
        .tiers
        .into_iter()
        .map(|tier| {
            let exclusion = providers
                .get(&tier.provider_id)
                .and_then(|p| routing::fallback_exclusion_with(&state.db, app_type, p, &blocked));
            let circuit_open = tier.breaker_state.as_deref() == Some("open")
                && tier.breaker_reopen_in_secs.is_some_and(|s| s > 0);
            // 屏蔽是用户显式动作：当前档被屏蔽也照常显示原因（其余排除不压过当前档）。
            // 不存在「位于当前档之前」这类位置性跳过：重新路由从链头扫，
            // 位置不是跳过理由（2026-09-19 用户定调）。
            let skip_reason = if !supports_proxy {
                None
            } else if exclusion == Some("blocked") {
                Some("blocked".to_string())
            } else if tier.is_current && exclusion != Some("native_configuration") {
                None
            } else if let Some(reason) = exclusion {
                Some(reason.to_string())
            } else if enabled && current_official_account {
                Some("current_official_account".to_string())
            } else if circuit_open {
                Some("circuit_open".to_string())
            } else {
                None
            };
            let error_rate = stats.get(&tier.provider_id).copied();
            let subscription_windows = providers
                .get(&tier.provider_id)
                .map(|p| {
                    crate::relay::provision::subscription_windows_from_settings(&p.settings_config)
                })
                .unwrap_or_default();
            let next_reset_at = crate::relay::tier_windows::next_reset_at(&subscription_windows);
            ApplicationRoutingTier {
                can_failover: supports_proxy
                    && providers.get(&tier.provider_id).is_some_and(|p| {
                        crate::proxy::provider_router::provider_supports_failover(app_type, p)
                    }),
                can_verify_models: verification_supported
                    && providers
                        .get(&tier.provider_id)
                        .is_some_and(|p| crate::relay::is_managed(&p.id)),
                models: providers
                    .get(&tier.provider_id)
                    .map(crate::proxy::auto_strategy::tier_models)
                    .unwrap_or_default(),
                subscription_windows,
                next_reset_at,
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
        chain_ids: chain,
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
pub async fn set_application_tier_blocked(
    state: tauri::State<'_, AppState>,
    app_type: String,
    provider_id: String,
    blocked: bool,
) -> Result<(), String> {
    routing::set_tier_blocked(&state.db, &app_type, &provider_id, blocked)
        .map_err(|e| e.to_string())
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

    /// 工作台表格按 camelCase 读 `canVerifyModels`（蛇形键会让前端拿到
    /// `undefined` → 验证按钮静默消失）。与 rows.rs 的 TierInfo 契约测试同款。
    #[test]
    fn application_routing_tier_serializes_can_verify_models_camel_case() {
        let tier = super::ApplicationRoutingTier {
            tier: super::TierBoardTier {
                provider_id: "loongport-0123456789abcdef".into(),
                name: "站 · pro池".into(),
                position: 1,
                is_current: false,
                rate_multiplier: Some(1.0),
                unit_price_per_million: None,
                effective_model: Some("gpt-5.6-sol".into()),
                avg_first_token_ms: None,
                balance_usd: None,
                verification_verdict: None,
                is_healthy: Some(true),
                consecutive_failures: Some(0),
                last_error: None,
                today_cost_usd: None,
                today_requests: None,
                cache_hit_rate: None,
                recent_activity: None,
                breaker_state: None,
                breaker_reopen_in_secs: None,
                affinity_remaining_secs: None,
            },
            skip_reason: None,
            error_rate: None,
            can_failover: true,
            can_verify_models: true,
            models: vec!["gpt-5.6-sol".into(), "gpt-5.5".into()],
            subscription_windows: vec![crate::relay::tier_windows::SubscriptionWindow {
                kind: crate::relay::tier_windows::WindowKind::Daily,
                limit_usd: 500.0,
                used_usd: Some(31.0),
                reset_at: Some(1_789_864_000),
            }],
            next_reset_at: Some(1_789_864_000),
        };
        let json = serde_json::to_value(&tier).expect("要能序列化");
        let obj = json.as_object().expect("是个对象");
        assert_eq!(
            obj.get("canVerifyModels").and_then(|v| v.as_bool()),
            Some(true),
            "验证资格必须以 camelCase 随档位下发，实际键：{:?}",
            obj.keys().collect::<Vec<_>>()
        );
        assert!(!obj.contains_key("can_verify_models"));
        // 模型目录是工作台「分组支持」筛选的数据源：必须以数组随档位下发。
        assert_eq!(
            obj.get("models").and_then(|v| v.as_array()).map(Vec::len),
            Some(2),
            "模型目录必须以 models 数组下发，实际键：{:?}",
            obj.keys().collect::<Vec<_>>()
        );
        // 订阅窗口与「下次重置」是工作台重置列的数据源：camelCase 随档位下发。
        let windows = obj
            .get("subscriptionWindows")
            .and_then(|v| v.as_array())
            .expect("subscriptionWindows 必须以数组下发");
        assert_eq!(windows.len(), 1);
        assert_eq!(
            windows[0].get("kind").and_then(|v| v.as_str()),
            Some("daily"),
            "窗口 kind 以 camelCase 枚举下发"
        );
        assert_eq!(
            windows[0].get("limitUsd").and_then(|v| v.as_f64()),
            Some(500.0)
        );
        assert_eq!(
            obj.get("nextResetAt").and_then(|v| v.as_i64()),
            Some(1_789_864_000)
        );
        assert!(!obj.contains_key("next_reset_at"));
    }
}
