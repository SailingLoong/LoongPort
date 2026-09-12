//! Compatibility commands and TierBoard facts for application routing.
//! Priority and fallback mutations belong to `proxy::application_routing`.

use crate::proxy::auto_strategy;
use crate::store::AppState;
use std::str::FromStr;

/// 自动模式状态快照（前端一次拉全）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoModeStatus {
    pub enabled: bool,
    /// "cheapest" | "fastest"
    pub strategy: String,
    /// 模型偏好（`None` = 不限）。
    pub model: Option<String>,
    /// 可选模型清单（该应用全部托管档位模型目录的并集；空 = 没有目录）。
    pub available_models: Vec<String>,
    /// 有没有可用的托管档位（与 [`set_auto_mode_enabled`] 的开启判据同源）。
    /// 总开关据此只对有档位的 app 生效，前端也用它把无档位卡的开关灰掉。
    pub has_candidates: bool,
    /// 该 CLI 的配置文件是否已存在（= CLI 装过/初始化过）。接管要改写这些
    /// 文件，不存在时开启必失败 —— 总开关只统计「档位 + CLI 都齐」的 app，
    /// 否则永远差一个「开不了」的，开关反复弹回（2026-08-17 实测症状）。
    pub cli_installed: bool,
}

fn require_auto_mode_app(app_type: &str) -> Result<(), String> {
    let app = crate::app_config::AppType::from_str(app_type)
        .map_err(|error| format!("无效的应用类型: {error}"))?;
    if !app.supports_local_proxy() {
        return Err(format!("{} 不支持自动模式", app.as_str()));
    }
    Ok(())
}

/// Whether the application has any configured routing entries.
fn has_routing_candidates(state: &crate::store::AppState, app_type: &str) -> bool {
    crate::proxy::application_routing::ordered_providers(&state.db, app_type)
        .map(|providers| !providers.is_empty())
        .unwrap_or(false)
}

/// CLI 配置文件是否存在。与接管路径的报错判据（「Gemini .env 文件不存在」
/// 「Grok Build 配置文件不存在」）同一批路径 —— 接管改写的就是这些文件。
fn cli_config_present(app_type: &str) -> bool {
    match crate::app_config::AppType::from_str(app_type) {
        Ok(crate::app_config::AppType::Claude) => {
            crate::config::get_claude_settings_path().exists()
        }
        Ok(crate::app_config::AppType::Codex) => {
            crate::codex_config::get_codex_config_path().exists()
        }
        Ok(crate::app_config::AppType::Gemini) => {
            crate::gemini_config::get_gemini_env_path().exists()
        }
        Ok(crate::app_config::AppType::GrokBuild) => {
            crate::grok_config::get_grok_config_path().exists()
        }
        _ => false,
    }
}

/// 读取某应用的自动模式状态
#[tauri::command]
pub async fn get_auto_mode_status(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<AutoModeStatus, String> {
    require_auto_mode_app(&app_type)?;
    let providers = state
        .db
        .get_all_providers(&app_type)
        .map_err(|e| e.to_string())?;
    Ok(AutoModeStatus {
        enabled: crate::proxy::application_routing::failover_enabled(&state.db, &app_type)
            .map_err(|e| e.to_string())?,
        strategy: auto_strategy::get_strategy(&state.db).as_str().to_string(),
        model: crate::proxy::application_routing::effective_model(&state.db, &app_type),
        available_models: auto_strategy::auto_mode_models(&providers),
        has_candidates: has_routing_candidates(&state, &app_type),
        cli_installed: cli_config_present(&app_type),
    })
}

/// Legacy toggle delegates to application fallback permission without switching.
#[tauri::command]
pub async fn set_auto_mode_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_type: String,
    enabled: bool,
) -> Result<(), String> {
    let _ = app;
    crate::proxy::application_routing::set_failover(&state.db, &app_type, enabled)
        .await
        .map_err(|e| e.to_string())
}

/// Policy reranking is retired; callers must submit an explicit priority order.
#[tauri::command]
pub async fn set_auto_mode_strategy(
    state: tauri::State<'_, AppState>,
    strategy: String,
) -> Result<(), String> {
    let _ = (state, strategy);
    Err("Policy routing has been retired; set application priority instead".into())
}

/// Set model intent without changing the selected provider.
#[tauri::command]
pub async fn set_auto_mode_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_type: String,
    model: Option<String>,
) -> Result<(), String> {
    set_auto_mode_model_impl(app, &state, &app_type, model.as_deref()).await
}

/// Shared model mutation for the application API and legacy tray events.
pub(crate) async fn set_auto_mode_model_impl(
    app: tauri::AppHandle,
    state: &AppState,
    app_type: &str,
    model: Option<&str>,
) -> Result<(), String> {
    let _ = app;
    if !crate::proxy::application_routing::takeover_enabled(&state.db, app_type)
        .map_err(|e| e.to_string())?
        || !state.proxy_service.is_running().await
    {
        return Err("Model routing requires a running application proxy and takeover".into());
    }
    crate::proxy::application_routing::set_model(&state.db, app_type, model)
        .map_err(|e| e.to_string())
}

/// Only persistent manual ordering remains supported.
#[tauri::command]
pub async fn set_easy_mode_mode(
    state: tauri::State<'_, AppState>,
    app_type: String,
    mode: String,
) -> Result<(), String> {
    if mode != "manual" {
        return Err("Policy routing has been retired; set application priority instead".into());
    }
    crate::proxy::application_routing::migrate(&state.db, &app_type).map_err(|e| e.to_string())
}

/// Compatibility alias for application priority.
#[tauri::command]
pub async fn set_easy_mode_manual_order(
    state: tauri::State<'_, AppState>,
    app_type: String,
    ordered_ids: Vec<String>,
) -> Result<(), String> {
    crate::proxy::application_routing::set_order(&state.db, &app_type, &ordered_ids)
        .map_err(|e| e.to_string())
}

/// 省心模式档位看板的一行（首页省心视图的展示事实，全部后端算好）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierBoardTier {
    pub provider_id: String,
    pub name: String,
    /// 展示序 = 纯策略序（自动）或用户手动序；不含选路时的会话亲和置顶 ——
    /// 当前档位靠 `is_current` 徽章表达，不靠排到第一。
    pub position: usize,
    pub is_current: bool,
    pub rate_multiplier: Option<f64>,
    /// 该档位有效模型的单价（每百万 token 输入+输出之和，美元）；
    /// `None` = 价格未知 —— 排序保守垫底的同一个事实，前端原样展示「未知」。
    pub unit_price_per_million: Option<f64>,
    pub effective_model: Option<String>,
    pub avg_first_token_ms: Option<u64>,
    /// 站点钱包余额（美元）。sub2api 用档位 sk 直查 `GET /v1/usage`（同站各档
    /// 共享一个账号钱包）；问不出时回落 one-api 系 billing 双端点（newapi 站），
    /// 仍问不到 → `None`，前端显示 —。
    pub balance_usd: Option<f64>,
    /// 模型验真合并判定（两源读侧合并、跨模型取最严重）。只上异常：
    /// "anomaly" | "suspicious"；Trusted/无报告 = `None`（被动监控不背书）。
    pub verification_verdict: Option<String>,
    /// 健康快照（`provider_health`；缺行时 DAO 合成默认健康行 —— 契约如此）。
    /// 从未失败 = `Some(true)` / `Some(0)` / `None`，前端据此不显示健康标记。
    pub is_healthy: Option<bool>,
    pub consecutive_failures: Option<u32>,
    /// Last recorded upstream failure, independent of routing eligibility.
    pub last_error: Option<String>,
    /// 今日花费（美元，本地时区「今天」，与限额页同口径）；`None` = 今天没有行。
    pub today_cost_usd: Option<f64>,
    /// 今日请求数（与 `today_cost_usd` 同一查询）。
    pub today_requests: Option<u64>,
    /// 7 天缓存命中率（0..1 分数，与首字耗时同窗口）；`None` = 无可判流量
    /// （分母为 0 时不显示，别把「不知道」当 0%）。
    pub cache_hit_rate: Option<f64>,
    /// 近 6 小时活动时间线（15 分钟一桶固定 24 桶，空桶补零）；
    /// `None` = 窗口内没有任何请求，前端不渲染时间线。
    pub recent_activity: Option<Vec<crate::services::usage_stats::ProviderActivityBucket>>,
    /// 内存熔断器状态（请求路径的真实闸门）：`"open"` | `"half_open"`；
    /// `None` = Closed 或代理未运行。致命打开但 DB 健康行还没到阈值的档位
    /// 靠这个上报（熔断器一次即开，DB 要攒阈值）。
    pub breaker_state: Option<String>,
    /// Open 状态距自动转 HalfOpen（探测）的剩余秒数；HalfOpen/Closed 无。
    pub breaker_reopen_in_secs: Option<u64>,
    /// 会话亲和剩余秒数（仅当前档位、亲和窗口内有流量时）——解释「更便宜的
    /// 档位为什么不马上接管」：中途换档丢提示词缓存，闲置后才重排。
    pub affinity_remaining_secs: Option<u64>,
}

/// 省心模式档位看板：首页省心视图一次拉全的聚合 DTO。
///
/// 业务事实（顺序/模式/策略/倍率/单价/耗时/命中/余额）的唯一源在后端，
/// 前端只渲染 —— 别在前端用多个原始命令各拼一遍（分叉温床）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierBoard {
    /// "auto" | "manual"
    pub mode: String,
    /// "cheapest" | "fastest"（全局一份）
    pub strategy: String,
    pub model: Option<String>,
    /// 可选模型清单（目录并集，顺序 = 档位序 → 目录内序），带每模型的
    /// 「几档可用 + 最便宜有效单价」——模型选择器的数据源。
    pub model_options: Vec<TierBoardModelOption>,
    pub current_provider_id: Option<String>,
    pub tiers: Vec<TierBoardTier>,
}

/// 模型选择器的一行：模型名 + 覆盖度（几档的目录含它）+ 最便宜有效单价
/// （倍率 × 模型单价，与排序同一套数据；价格/倍率未知 → `None`）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierBoardModelOption {
    pub model: String,
    pub tier_count: u32,
    pub cheapest_price_per_million: Option<f64>,
}

/// 省心模式档位看板（首页省心视图数据源）。
#[tauri::command]
pub async fn easy_mode_tier_board(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<TierBoard, String> {
    tier_board_impl(&state, &app_type).await
}

/// 看板核心（真实 smoke 直接调它，不走 tauri State）。
pub(crate) async fn tier_board_impl(state: &AppState, app_type: &str) -> Result<TierBoard, String> {
    crate::app_config::AppType::from_str(app_type).map_err(|e| e.to_string())?;
    let db = &state.db;
    let providers = db.get_all_providers(app_type).map_err(|e| e.to_string())?;
    let ranked = crate::proxy::application_routing::ordered_providers(db, app_type)
        .map_err(|e| e.to_string())?;
    let multipliers = db.get_tier_rate_multipliers(app_type).unwrap_or_default();
    let ttft = db
        .get_provider_avg_first_token_ms(app_type, chrono::Utc::now().timestamp() - 7 * 86400)
        .unwrap_or_default();
    let today = db.get_provider_today_stats(app_type).unwrap_or_default();
    let cache_hit_rates = db
        .get_provider_cache_hit_rates(app_type, chrono::Utc::now().timestamp() - 7 * 86400)
        .unwrap_or_default();
    let recent_activity = db
        .get_provider_activity_buckets(app_type, chrono::Utc::now().timestamp() - 6 * 3600, 900, 24)
        .unwrap_or_default();
    let routing_active = crate::proxy::application_routing::takeover_enabled(db, app_type)
        .map_err(|e| e.to_string())?
        && state.proxy_service.is_running().await;
    let model_pref = routing_active
        .then(|| crate::proxy::application_routing::effective_model(db, app_type))
        .flatten();
    let current_id = crate::proxy::application_routing::current_provider_id(db, app_type);

    let provider_ids: Vec<String> = ranked.iter().map(|p| p.id.clone()).collect();
    let breaker_states = state
        .proxy_service
        .provider_breaker_states(app_type, &ranked)
        .await;

    // 余额走缓存：看板纯读（模式/视图不驱动刷新 —— 触发点与红线论证收在
    // services::site_balance_refresh 与 relay::balance::spawn_stale_refresh）。
    // 此前这里是同步网络扇出 —— 20-30 家里一家超时型挂掉，整板就等它
    // 30-90s，且冷启动/窗口聚焦每次重演（用户反馈「每次打开软件省心模式
    // 卡好久」）。缓存键是站点+账号：同站两个账号的档位各查各的钱包。
    let (tier_keys, _site_keys) =
        crate::services::site_balance_refresh::tier_site_keys(app_type, &ranked);
    let cached = crate::relay::balance::cached_site_balances(db);
    let balances: std::collections::HashMap<String, Option<f64>> = tier_keys
        .iter()
        .map(|(tier_id, key)| {
            (
                tier_id.clone(),
                cached.get(key).map(|(balance, _)| *balance).unwrap_or(None),
            )
        })
        .collect();

    // 健康快照（缺行时 DAO 合成默认健康行，见 get_provider_health 的契约）
    let mut health: std::collections::HashMap<String, crate::proxy::types::ProviderHealth> =
        std::collections::HashMap::new();
    for p in &ranked {
        if let Ok(h) = db.get_provider_health(&p.id, app_type).await {
            health.insert(p.id.clone(), h);
        }
    }

    // 验真判定：跨模型聚合收在模型验证模块（worst_verdict_by_provider），
    // 这里只做 DTO 字符串映射；模块下线时聚合恒为空 ⇒ 看板无验真列。
    let verification =
        crate::relay::model_verification::store::worst_verdict_by_provider(db, &provider_ids)
            .unwrap_or_default();
    let verification_verdict = |provider_id: &str| -> Option<String> {
        verification
            .get(provider_id)
            .and_then(|verdict| match verdict {
                crate::relay::model_verification::types::Verdict::Anomaly => {
                    Some("anomaly".to_string())
                }
                crate::relay::model_verification::types::Verdict::Suspicious => {
                    Some("suspicious".to_string())
                }
                _ => None,
            })
    };

    let tiers = ranked
        .into_iter()
        .enumerate()
        .map(|(position, p)| {
            let effective_model = routing_active
                .then(|| crate::proxy::application_routing::model_for_provider(db, app_type, &p))
                .flatten()
                .or_else(|| {
                    crate::app_config::AppType::from_str(app_type)
                        .ok()
                        .and_then(|app| {
                            crate::relay::provision::selected_model(&app, &p.settings_config)
                        })
                });
            let unit_price_per_million = effective_model
                .as_deref()
                .and_then(|model| auto_strategy::effective_unit_price(db, &p, Some(model)));
            TierBoardTier {
                is_current: current_id.as_deref() == Some(p.id.as_str()),
                effective_model,
                unit_price_per_million,
                rate_multiplier: multipliers.get(&p.id).copied(),
                avg_first_token_ms: ttft.get(&p.id).copied(),
                balance_usd: balances.get(&p.id).copied().flatten(),
                verification_verdict: verification_verdict(&p.id),
                is_healthy: health.get(&p.id).map(|h| h.is_healthy),
                consecutive_failures: health.get(&p.id).map(|h| h.consecutive_failures),
                last_error: health.get(&p.id).and_then(|h| h.last_error.clone()),
                today_cost_usd: today.get(&p.id).map(|(cost, _)| *cost),
                today_requests: today.get(&p.id).map(|(_, requests)| *requests),
                cache_hit_rate: cache_hit_rates.get(&p.id).copied(),
                recent_activity: recent_activity.get(&p.id).cloned(),
                breaker_state: breaker_states
                    .get(&p.id)
                    .map(|snap| if snap.half_open { "half_open" } else { "open" }.to_string()),
                breaker_reopen_in_secs: breaker_states
                    .get(&p.id)
                    .and_then(|snap| snap.reopen_in_secs),
                affinity_remaining_secs: None,
                provider_id: p.id.clone(),
                name: p.name.clone(),
                position,
            }
        })
        .collect();

    // 模型选项：全部托管档的目录并集（不按偏好过滤——选择器清单不因当前
    // 偏好缩水），每模型统计覆盖档数与最便宜「倍率×单价」（与排序同源；
    // 倍率或价格未知的档不参与最低价比较）
    let mut model_options: Vec<TierBoardModelOption> = Vec::new();
    for provider in providers.values() {
        let multiplier = multipliers.get(&provider.id).copied();
        for model in auto_strategy::tier_models(provider) {
            let unit_price = model_unit_price(db, &model);
            match model_options
                .iter_mut()
                .find(|option| option.model == model)
            {
                Some(option) => {
                    option.tier_count += 1;
                    if let (Some(multiplier), Some(price)) = (multiplier, unit_price) {
                        option.cheapest_price_per_million = match option.cheapest_price_per_million
                        {
                            Some(current) => Some(current.min(multiplier * price)),
                            None => Some(multiplier * price),
                        };
                    }
                }
                None => model_options.push(TierBoardModelOption {
                    model: model.clone(),
                    tier_count: 1,
                    cheapest_price_per_million: match (multiplier, unit_price) {
                        (Some(multiplier), Some(price)) => Some(multiplier * price),
                        _ => None,
                    },
                }),
            }
        }
    }

    Ok(TierBoard {
        mode: "manual".to_string(),
        strategy: auto_strategy::get_strategy(db).as_str().to_string(),
        model: model_pref,
        model_options,
        current_provider_id: current_id,
        tiers,
    })
}

/// 模型单价（每百万 token 输入+输出之和，美元）；价表未收录 → `None`。
/// 与 `auto_strategy::tier_unit_price` 同一张表，只是按模型名直查。
fn model_unit_price(db: &crate::Database, model: &str) -> Option<f64> {
    let conn = db
        .conn
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (input, output, _cache_read, _cache_creation) =
        crate::services::usage_stats::find_model_pricing_row(&conn, model).ok()??;
    let input: f64 = input.parse().ok()?;
    let output: f64 = output.parse().ok()?;
    Some(input + output)
}

#[cfg(test)]
mod tests {
    use super::{require_auto_mode_app, tier_board_impl};
    use crate::store::AppState;
    use crate::Database;
    use serde_json::json;
    use serial_test::serial;
    use std::sync::Arc;

    #[test]
    fn auto_mode_rejects_apps_without_a_proxy_data_plane() {
        assert!(require_auto_mode_app("claude").is_ok());
        assert!(require_auto_mode_app("pi").is_err());
    }

    /// 看板余额走缓存（SWR）：种子缓存原样上板；缓存 TTL 内不触发任何刷新
    /// （零网络、看板命令毫秒级返回）。
    #[tokio::test]
    #[serial]
    async fn tier_board_reads_balances_from_cache_without_network() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let id = crate::relay::provision::provider_id_for("https://cache.example", Some(1), 1);
        let mut tier = crate::provider::Provider::with_id(
            id.clone(),
            "缓存档".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://cache.example",
                    "ANTHROPIC_AUTH_TOKEN": "sk-test-not-a-real-key",
                }
            }),
            None,
        );
        // 缓存键带账号维度：档位要带归属，看板才知道去 (origin, account) 哪条读
        tier.meta = Some(crate::provider::ProviderMeta {
            loongport_account_id: Some(1),
            ..Default::default()
        });
        db.save_provider("claude", &tier).unwrap();

        // 无缓存：余额 None（显示 —）
        let board = tier_board_impl(&state, "claude").await.unwrap();
        assert_eq!(board.tiers[0].balance_usd, None, "缓存未命中 → —");

        // 种子缓存（TTL 内）→ 上板；后台刷新因缓存新鲜而不触发（本测试零网络）
        let now = chrono::Utc::now().timestamp();
        crate::relay::balance::upsert_site_balance(
            &db,
            &("https://cache.example".to_string(), 1),
            (Some(12.34), now),
        )
        .unwrap();
        let board = tier_board_impl(&state, "claude").await.unwrap();
        assert_eq!(
            board.tiers[0].balance_usd,
            Some(12.34),
            "缓存值必须原样上板"
        );
    }

    /// 看板聚合：顺序=选路序、倍率/单价/耗时/命中齐全；手动模式反映手动序。
    /// 余额链对 http 端点零网络（真实站点路径由 ignored 的真实 smoke 覆盖）。
    #[tokio::test]
    #[serial]
    async fn tier_board_aggregates_display_facts() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let expensive = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let cheap = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        // http 端点（fetch_site_balances 只对 https 发请求）
        let tier = |id: &str, name: &str, config: serde_json::Value| {
            crate::provider::Provider::with_id(id.to_string(), name.to_string(), config, None)
        };
        db.save_provider(
            "claude",
            &tier(&expensive, "贵档", json!({ "config": "model = \"m-x\"\n" })),
        )
        .unwrap();
        db.save_provider(
            "claude",
            &tier(&cheap, "便宜档", json!({ "config": "model = \"m-x\"\n" })),
        )
        .unwrap();
        db.set_tier_rate_multiplier("claude", &expensive, Some(2.0))
            .unwrap();
        db.set_tier_rate_multiplier("claude", &cheap, Some(0.5))
            .unwrap();
        db.set_current_provider("claude", &expensive).unwrap();

        crate::proxy::application_routing::set_order(
            &db,
            "claude",
            &[cheap.clone(), expensive.clone()],
        )
        .unwrap();
        let board = tier_board_impl(&state, "claude").await.unwrap();
        assert_eq!(board.mode, "manual");
        assert_eq!(board.strategy, "cheapest");
        assert_eq!(board.tiers.len(), 2);
        assert_eq!(board.tiers[0].provider_id, cheap, "自动模式便宜在前");
        assert_eq!(board.tiers[0].rate_multiplier, Some(0.5));
        assert_eq!(
            board.tiers[0].unit_price_per_million, None,
            "价表没收录 → 未知"
        );
        assert!(
            board
                .tiers
                .iter()
                .any(|t| t.is_current && t.provider_id == expensive),
            "当前档位有命中标记"
        );

        // 验真 verdict：被动异常上板、active Trusted 不上板（只报异常不背书）
        use crate::relay::model_verification::{
            store::{list_for_provider_ids, upsert_active, upsert_passive},
            types::{TargetKey, Verdict, VerificationReport, RULES_VERSION},
        };
        let verification_report =
            |provider_id: &str, model: &str, verdict: Verdict| VerificationReport {
                target: TargetKey::new(provider_id, "claude", model),
                verdict,
                evidence_level:
                    crate::relay::model_verification::types::EvidenceLevel::ProtocolBehavior,
                facts: Vec::new(),
                diagnostics: Vec::new(),
                rules_version: RULES_VERSION,
                checked_at: 1_700_000_000,
            };
        upsert_passive(&db, &verification_report(&cheap, "m-x", Verdict::Anomaly)).unwrap();
        upsert_active(
            &db,
            &verification_report(&expensive, "m-x", Verdict::Trusted),
        )
        .unwrap();
        let board = tier_board_impl(&state, "claude").await.unwrap();
        let by_id = |id: &str| board.tiers.iter().find(|t| t.provider_id == id).unwrap();
        assert_eq!(
            by_id(&cheap).verification_verdict.as_deref(),
            Some("anomaly"),
            "被动异常必须上板"
        );
        assert_eq!(
            by_id(&expensive).verification_verdict,
            None,
            "active Trusted 不上板"
        );
        assert_eq!(list_for_provider_ids(&db, &[]).unwrap().len(), 0);

        crate::proxy::application_routing::set_order(
            &db,
            "claude",
            &[expensive.clone(), cheap.clone()],
        )
        .unwrap();
        let board = tier_board_impl(&state, "claude").await.unwrap();
        assert_eq!(board.mode, "manual");
        assert_eq!(board.tiers[0].provider_id, expensive, "手动序优先");
    }

    /// ⭐ 看板是纯策略序：选路的会话亲和置顶只在请求时发生，展示不打乱顺序 ——
    /// 当前档位靠 `is_current` 徽章表达，不靠排到第一。用户读看板的心智模型是
    /// 「价格序 + 谁在用标当前 + 没在用的给出原因」。
    #[tokio::test]
    #[serial]
    async fn tier_board_keeps_priority_with_active_current() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let expensive = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let cheap = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        let tier = |id: &str| {
            crate::provider::Provider::with_id(
                id.to_string(),
                id.to_string(),
                json!({ "config": "model = \"m-x\"\n" }),
                None,
            )
        };
        db.save_provider("claude", &tier(&expensive)).unwrap();
        db.save_provider("claude", &tier(&cheap)).unwrap();
        db.set_tier_rate_multiplier("claude", &expensive, Some(2.0))
            .unwrap();
        db.set_tier_rate_multiplier("claude", &cheap, Some(0.5))
            .unwrap();
        db.set_current_provider("claude", &expensive).unwrap();

        // 当前档位（贵）30 分钟内有流量 → 选路会亲和置顶；看板必须保持纯价格序
        seed_board_activity(&db, "claude", &expensive);
        crate::proxy::application_routing::set_order(
            &db,
            "claude",
            &[cheap.clone(), expensive.clone()],
        )
        .unwrap();

        let board = tier_board_impl(&state, "claude").await.unwrap();
        assert_eq!(
            board.tiers[0].provider_id, cheap,
            "看板第一张是最便宜的，不被亲和置顶顶走"
        );
        let current = board.tiers.iter().find(|t| t.is_current).unwrap();
        assert_eq!(current.provider_id, expensive);
        assert_eq!(current.position, 1, "当前徽章在它自己的价格位上");
    }

    /// ⭐ 失败原因透出：`provider_health` 的健康态/连续失败/`last_error`（上游
    /// 报错原文）跟着看板走，给「为什么不选用」的标签用；没失败过的档位全 None。
    #[tokio::test]
    #[serial]
    async fn tier_board_surfaces_provider_health_for_failed_tiers() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let dead = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let fine = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        let tier = |id: &str| {
            crate::provider::Provider::with_id(
                id.to_string(),
                id.to_string(),
                json!({ "config": "model = \"m-x\"\n" }),
                None,
            )
        };
        db.save_provider("claude", &tier(&dead)).unwrap();
        db.save_provider("claude", &tier(&fine)).unwrap();

        db.update_provider_health_with_threshold(
            &dead,
            "claude",
            false,
            Some("上游 HTTP 403: {\"error\":{\"message\":\"无可用渠道\"}}".to_string()),
            1,
        )
        .await
        .unwrap();

        let board = tier_board_impl(&state, "claude").await.unwrap();
        let by_id = |id: &str| board.tiers.iter().find(|t| t.provider_id == id).unwrap();
        assert_eq!(by_id(&dead).is_healthy, Some(false));
        assert_eq!(by_id(&dead).consecutive_failures, Some(1));
        assert!(
            by_id(&dead)
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("403")),
            "上游报错原文必须原样透出"
        );
        assert_eq!(
            by_id(&fine).is_healthy,
            Some(true),
            "没失败过的档位 = 健康（缺行时 DAO 合成默认健康行）"
        );
        assert_eq!(by_id(&fine).consecutive_failures, Some(0));
        assert_eq!(by_id(&fine).last_error, None);
    }

    /// ⭐ 倒计时双字段：亲和剩余只在「当前档位 + 窗口内有流量」时给出；
    /// 熔断字段在代理未运行时全 None（无内存态可读，前端只信 DB 健康）。
    #[tokio::test]
    #[serial]
    async fn tier_board_does_not_imply_automatic_switchback_after_idle() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let current = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let other = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        let tier = |id: &str| {
            crate::provider::Provider::with_id(
                id.to_string(),
                id.to_string(),
                json!({ "config": "model = \"m-x\"\n" }),
                None,
            )
        };
        db.save_provider("claude", &tier(&current)).unwrap();
        db.save_provider("claude", &tier(&other)).unwrap();
        db.set_current_provider("claude", &current).unwrap();
        // 同一时刻给「别的档位」也 seed 流量：亲和剩余只跟当前档位自己的
        // 最近流量走，别的档位有流量也不给倒计时
        seed_board_usage(
            &db,
            "claude",
            &current,
            "aff-cur",
            0.0,
            200,
            0,
            0,
            0,
            Some(10),
            -60,
        );
        seed_board_usage(
            &db,
            "claude",
            &other,
            "aff-oth",
            0.0,
            200,
            0,
            0,
            0,
            Some(10),
            -60,
        );

        let board = tier_board_impl(&state, "claude").await.unwrap();
        let by_id = |id: &str| board.tiers.iter().find(|t| t.provider_id == id).unwrap();
        assert_eq!(by_id(&current).affinity_remaining_secs, None);
        assert_eq!(
            by_id(&other).affinity_remaining_secs,
            None,
            "非当前档位不给亲和倒计时"
        );
        assert_eq!(
            by_id(&current).breaker_state,
            None,
            "代理未运行 → 无内存熔断态"
        );
        assert_eq!(by_id(&current).breaker_reopen_in_secs, None);
    }

    /// Recent usage must not reorder persistent application priorities.
    fn seed_board_activity(db: &Database, app_type: &str, provider_id: &str) {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model,
                input_tokens, output_tokens, total_cost_usd,
                latency_ms, first_token_ms, status_code, created_at
            ) VALUES (?1, ?2, ?3, 'm', 1, 1, '0', 10, 10, 200, ?4)",
            rusqlite::params![
                format!("board-{provider_id}"),
                provider_id,
                app_type,
                chrono::Utc::now().timestamp(),
            ],
        )
        .unwrap();
    }

    /// ⭐ 今日运行事实（本地时区口径）+ 7 天缓存命中率：昨日行不计入今日；
    /// 缓存率 = cache_read /（fresh_input + cache_creation + cache_read）；
    /// 没有可判流量（分母 0）或没有行的档位 → `None`，不显示误导性 0%。
    #[tokio::test]
    #[serial]
    async fn tier_board_surfaces_today_stats_and_cache_hit_rate() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let busy = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let cold = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        let tier = |id: &str| {
            crate::provider::Provider::with_id(
                id.to_string(),
                id.to_string(),
                json!({ "config": "model = \"m-x\"\n" }),
                None,
            )
        };
        db.save_provider("claude", &tier(&busy)).unwrap();
        db.save_provider("claude", &tier(&cold)).unwrap();

        // 今天两行：$0.5 + $0.25；fresh input 10、cache read 90（claude 语义
        // input_tokens=fresh，semantics 缺省 0=legacy 也按 fresh 归一）
        seed_board_usage(
            &db,
            "claude",
            &busy,
            "busy-t1",
            0.5,
            200,
            10,
            0,
            90,
            Some(10),
            0,
        );
        seed_board_usage(
            &db,
            "claude",
            &busy,
            "busy-t2",
            0.25,
            200,
            10,
            0,
            90,
            Some(10),
            0,
        );
        // 昨天一行（now−2 天，双时区安全）：$9 不计入今日；token 全 0 不进缓存率
        seed_board_usage(
            &db,
            "claude",
            &busy,
            "busy-y1",
            9.0,
            200,
            0,
            0,
            0,
            Some(10),
            -2 * 86400,
        );
        // cold 档没有任何行

        let board = tier_board_impl(&state, "claude").await.unwrap();
        let by_id = |id: &str| board.tiers.iter().find(|t| t.provider_id == id).unwrap();
        let busy_tier = by_id(&busy);
        assert_eq!(busy_tier.today_cost_usd, Some(0.75), "昨日的 $9 不计入");
        assert_eq!(busy_tier.today_requests, Some(2));
        // 7 天窗口：read 180 / (fresh 20 + creation 0 + read 180) = 0.9
        assert!((busy_tier.cache_hit_rate.unwrap() - 0.9).abs() < 1e-9);
        let cold_tier = by_id(&cold);
        assert_eq!(cold_tier.today_cost_usd, None, "没有行 → None，前端显示 —");
        assert_eq!(cold_tier.today_requests, None);
        assert_eq!(cold_tier.cache_hit_rate, None, "分母为 0 不显示 0%");
    }

    /// ⭐ 模型选项：目录并集带「几档可用 + 最便宜倍率×单价」；价表未收录或
    /// 倍率未知的模型不给最低价（不猜）。
    #[tokio::test]
    #[serial]
    async fn tier_board_model_options_carry_coverage_and_cheapest_price() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let cheap = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let expensive = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        let with_catalog = |id: &str, models: &[&str]| {
            crate::provider::Provider::with_id(
                id.to_string(),
                id.to_string(),
                json!({ "modelCatalog": { "models": models.iter().map(|m| json!({ "model": m })).collect::<Vec<_>>() } }),
                None,
            )
        };
        db.save_provider("codex", &with_catalog(&cheap, &["m-x", "m-y"]))
            .unwrap();
        db.save_provider("codex", &with_catalog(&expensive, &["m-x"]))
            .unwrap();
        db.set_available_models("codex", &cheap, &["m-x".into(), "m-y".into()])
            .unwrap();
        db.set_available_models("codex", &expensive, &["m-x".into()])
            .unwrap();
        db.set_tier_rate_multiplier("codex", &cheap, Some(0.5))
            .unwrap();
        db.set_tier_rate_multiplier("codex", &expensive, Some(2.0))
            .unwrap();
        // 价表只收录 m-x：$1 输入 + $1 输出 = 单价 2
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO model_pricing (model_id, display_name, input_cost_per_million, output_cost_per_million)
                 VALUES ('m-x', 'M X', '1', '1')",
                [],
            )
            .unwrap();
        }

        let board = tier_board_impl(&state, "codex").await.unwrap();
        let by_model = |model: &str| {
            board
                .model_options
                .iter()
                .find(|option| option.model == model)
                .unwrap_or_else(|| panic!("缺 {model}"))
        };
        let m_x = by_model("m-x");
        assert_eq!(m_x.tier_count, 2, "两档目录都含 m-x");
        // 最低 = min(0.5×2, 2.0×2) = 1.0
        assert!((m_x.cheapest_price_per_million.unwrap() - 1.0).abs() < 1e-9);
        let m_y = by_model("m-y");
        assert_eq!(m_y.tier_count, 1);
        assert_eq!(
            m_y.cheapest_price_per_million, None,
            "价表未收录 → 不给最低价"
        );
    }

    /// ⭐ 近期活动时间线：6 小时 / 15 分钟一桶固定 24 桶，每桶成功数/失败数/
    /// 均首字；窗外行不计；窗口内没有任何行的档位 → `None`（前端不渲染时间线）。
    #[tokio::test]
    #[serial]
    async fn tier_board_surfaces_recent_activity_buckets() {
        let _home = test_home();
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone());

        let busy = crate::relay::provision::provider_id_for("https://a.example", Some(1), 1);
        let cold = crate::relay::provision::provider_id_for("https://b.example", Some(1), 2);
        let tier = |id: &str| {
            crate::provider::Provider::with_id(
                id.to_string(),
                id.to_string(),
                json!({ "config": "model = \"m-x\"\n" }),
                None,
            )
        };
        db.save_provider("claude", &tier(&busy)).unwrap();
        db.save_provider("claude", &tier(&cold)).unwrap();

        // 桶 0（窗口最早期）：两次成功 ttft 100/300 → 均 200
        seed_board_usage(
            &db,
            "claude",
            &busy,
            "act-b0-a",
            0.0,
            200,
            0,
            0,
            0,
            Some(100),
            -6 * 3600 + 120,
        );
        seed_board_usage(
            &db,
            "claude",
            &busy,
            "act-b0-b",
            0.0,
            200,
            0,
            0,
            0,
            Some(300),
            -6 * 3600 + 240,
        );
        // 桶 23（最近）：一次失败（403，无首字）
        seed_board_usage(
            &db,
            "claude",
            &busy,
            "act-b23-f",
            0.0,
            403,
            0,
            0,
            0,
            None,
            -60,
        );
        // 窗外（8 小时前）：不计入
        seed_board_usage(
            &db,
            "claude",
            &busy,
            "act-out",
            0.0,
            200,
            0,
            0,
            0,
            Some(10),
            -8 * 3600,
        );

        let board = tier_board_impl(&state, "claude").await.unwrap();
        let by_id = |id: &str| board.tiers.iter().find(|t| t.provider_id == id).unwrap();
        let activity = by_id(&busy)
            .recent_activity
            .as_ref()
            .expect("窗口内有行的档位必须有活动桶");
        assert_eq!(activity.len(), 24, "固定 24 桶（空桶补零），时间线不断续");
        assert_eq!(activity[0].success_count, 2);
        assert_eq!(activity[0].fail_count, 0);
        assert_eq!(activity[0].avg_first_token_ms, Some(200));
        assert_eq!(activity[23].success_count, 0);
        assert_eq!(activity[23].fail_count, 1);
        assert_eq!(activity[23].avg_first_token_ms, None);
        assert_eq!(activity[5].success_count, 0, "空桶补零");
        assert_eq!(activity[5].avg_first_token_ms, None);
        assert!(
            by_id(&cold).recent_activity.is_none(),
            "窗口内没有行的档位 → None，前端不渲染"
        );
    }

    /// 看板用量 seed：一行带花费与缓存 token 的明细（created_at = now + 偏移秒）。
    #[allow(clippy::too_many_arguments)]
    fn seed_board_usage(
        db: &Database,
        app_type: &str,
        provider_id: &str,
        request_id: &str,
        cost_usd: f64,
        status_code: i64,
        input_tokens: i64,
        cache_creation: i64,
        cache_read: i64,
        first_token_ms: Option<i64>,
        created_at_offset_secs: i64,
    ) {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model,
                input_tokens, cache_creation_tokens, cache_read_tokens,
                total_cost_usd, latency_ms, first_token_ms, status_code, created_at
            ) VALUES (?1, ?2, ?3, 'm', ?4, ?5, ?6, ?7, 10, ?8, ?9, ?10)",
            rusqlite::params![
                request_id,
                provider_id,
                app_type,
                input_tokens,
                cache_creation,
                cache_read,
                cost_usd,
                first_token_ms,
                status_code,
                chrono::Utc::now().timestamp() + created_at_offset_secs,
            ],
        )
        .unwrap();
    }

    fn test_home() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());
        std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
        crate::settings::reload_settings().unwrap();
        dir
    }
}
