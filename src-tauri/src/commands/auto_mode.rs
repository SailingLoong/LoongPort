//! 自动模式命令（LoongPort）。
//!
//! 自动模式：用户只选 app（和模型，M3），系统按全局策略（价格最低默认 /
//! 响应最快）从托管档位里自动挑最合适的，当前档位带会话亲和。
//! 选路注入在 `proxy::provider_router::select_providers`，这里只负责开关、
//! 策略与「开启即切到策略第一名」的编排。

use crate::events::PROVIDER_SWITCHED;
use crate::proxy::auto_strategy::{self, AutoStrategy};
use crate::store::AppState;
use std::str::FromStr;
use tauri::Emitter;

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

/// 有没有可用的托管档位。与 `set_auto_mode_enabled` 里的开启判据同一份实现
/// （`rank_managed_tier_candidates`），别各写一个判据 —— 分叉的结局是
/// 「状态说能开、开启却报错」。
fn has_managed_candidates(db: &crate::store::AppState, app_type: &str) -> bool {
    auto_strategy::rank_managed_tier_candidates(&db.db, app_type, true)
        .map(|ranked| ranked.is_some_and(|candidates| !candidates.is_empty()))
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
        enabled: auto_strategy::is_auto_mode_enabled(&state.db, &app_type),
        strategy: auto_strategy::get_strategy(&state.db).as_str().to_string(),
        model: auto_strategy::get_model_pref(&state.db, &app_type),
        available_models: auto_strategy::auto_mode_models(&providers),
        has_candidates: has_managed_candidates(&state, &app_type),
        cli_installed: cli_config_present(&app_type),
    })
}

/// 设置某应用的自动模式开关。
///
/// 开启要求该应用已处于代理接管态（与故障转移同一条前置：自动切换只发生在
/// 接管态，CLI 流量走本地代理，热切换无感）。开启成功后立即切到策略第一名，
/// 让「开了自动模式」的语义当场兑现；关闭只落开关，不动当前供应商。
#[tauri::command]
pub async fn set_auto_mode_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_type: String,
    enabled: bool,
) -> Result<(), String> {
    require_auto_mode_app(&app_type)?;
    log::info!("[AutoMode] Setting enabled: app_type='{app_type}', enabled={enabled}");

    if enabled {
        let config = state
            .db
            .get_proxy_config_for_app(&app_type)
            .await
            .map_err(|e| e.to_string())?;
        if !config.enabled {
            return Err("需要先启用该应用的代理接管，再开启自动模式".to_string());
        }

        // 候选必须非空才允许开 —— 空开会在 select_providers 里静默回退常规选路，
        // 用户以为开了自动模式实际没生效。
        // 排序与选路共用同一份实现（auto_strategy::rank_managed_tier_candidates，
        // 含会话亲和置顶）：活跃会话里第一名就是当前档位，切换为 no-op，不丢缓存。
        let Some(ranked) = auto_strategy::rank_managed_tier_candidates(&state.db, &app_type, true)
            .map_err(|e| e.to_string())?
        else {
            return Err(
                "没有可用的托管档位，无法开启自动模式。请先在中转站区登录并获取档位。".to_string(),
            );
        };

        if let Some(best) = ranked.first() {
            let best_id = best.id.clone();
            let current_id = auto_strategy::effective_current_provider_id(&state.db, &app_type);
            if current_id.as_deref() != Some(best_id.as_str()) {
                state
                    .proxy_service
                    .switch_proxy_target(&app_type, &best_id)
                    .await
                    .map_err(|e| e.to_string())?;

                let _ = app.emit(
                    PROVIDER_SWITCHED,
                    serde_json::json!({
                        "appType": app_type,
                        "providerId": best_id,
                        "source": "autoModeEnabled"
                    }),
                );
            }
        }
    }

    auto_strategy::set_enabled(&state.db, &app_type, enabled).map_err(|e| e.to_string())?;

    // 刷新托盘菜单，确保状态同步
    if let Ok(new_menu) = crate::tray::create_tray_menu(&app, &state) {
        if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
            let _ = tray.set_menu(Some(new_menu));
        }
    }

    Ok(())
}

/// 设置全局策略（cheapest / fastest）。
///
/// 只落设置不主动切换：新策略从下一批请求的排序生效（会话亲和仍然优先，
/// 活跃会话不会被策略切换打断 —— 那正是亲和规则存在的理由）。
#[tauri::command]
pub async fn set_auto_mode_strategy(
    state: tauri::State<'_, AppState>,
    strategy: String,
) -> Result<(), String> {
    let parsed = match strategy.as_str() {
        "cheapest" => AutoStrategy::Cheapest,
        "fastest" => AutoStrategy::Fastest,
        other => return Err(format!("未知的自动模式策略: {other}")),
    };
    auto_strategy::set_strategy(&state.db, parsed).map_err(|e| e.to_string())
}

/// 设置某应用的自动模式模型偏好（M3 托盘 app→模型 映射的落点）。
///
/// `model = None` 表示「不限模型」。点选模型是**显式**选择：绕过会话亲和立即
/// 切到「目录含该模型、策略最优」的档位（亲和保护的是系统重排别打断会话，
/// 不替用户拒绝他刚点的选择），并把该档位的选中模型对齐到偏好。
#[tauri::command]
pub async fn set_auto_mode_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_type: String,
    model: Option<String>,
) -> Result<(), String> {
    set_auto_mode_model_impl(app, &state, &app_type, model.as_deref()).await
}

/// `set_auto_mode_model` 的可从托盘调用的核心（托盘事件处理拿不到
/// `tauri::State`，但有 `AppHandle`）。
pub(crate) async fn set_auto_mode_model_impl(
    app: tauri::AppHandle,
    state: &AppState,
    app_type: &str,
    model: Option<&str>,
) -> Result<(), String> {
    require_auto_mode_app(app_type)?;
    if !auto_strategy::is_auto_mode_enabled(&state.db, app_type) {
        return Err("自动模式未开启，请先在设置中开启再选择模型".to_string());
    }
    let config = state
        .db
        .get_proxy_config_for_app(app_type)
        .await
        .map_err(|e| e.to_string())?;
    if !config.enabled {
        return Err("自动模式需要代理接管态，请先恢复接管".to_string());
    }

    auto_strategy::set_model_pref(&state.db, app_type, model).map_err(|e| e.to_string())?;

    // 显式选择：绕过亲和（honor_affinity=false），立即切到过滤+排序后的第一名
    let ranked = auto_strategy::rank_managed_tier_candidates(&state.db, app_type, false)
        .map_err(|e| e.to_string())?;
    let Some(best) = ranked.and_then(|mut r| {
        if r.is_empty() {
            None
        } else {
            Some(r.remove(0))
        }
    }) else {
        return Ok(()); // 偏好已落库；没有可切档位时下一次选路自然生效
    };

    let current_id = auto_strategy::effective_current_provider_id(&state.db, app_type);
    if current_id.as_deref() != Some(best.id.as_str()) {
        state
            .proxy_service
            .switch_proxy_target(app_type, &best.id)
            .await
            .map_err(|e| e.to_string())?;
        let _ = app.emit(
            PROVIDER_SWITCHED,
            serde_json::json!({
                "appType": app_type,
                "providerId": best.id,
                "source": "autoModeModel"
            }),
        );
    }

    // 对齐档位的选中模型（接管态下走热路径，无 ChatGPT 退重开编排）
    if let Some(model) = model {
        let wants_model = auto_strategy::tier_models(&best).iter().any(|m| m == model);
        let current_model = crate::relay::provision::extract_model(&best.settings_config);
        if wants_model && current_model.as_deref() != Some(model) {
            let mut user_choice = None;
            loop {
                let outcome = crate::commands::switch_tier_model_command(
                    &app,
                    &best.id,
                    crate::app_config::AppType::from_str(app_type).map_err(|e| e.to_string())?,
                    model,
                    user_choice,
                )
                .await
                .map_err(|e| e.to_string())?;
                match outcome {
                    crate::commands::SwitchTierCommandResult::ConfirmationRequired {
                        target_name,
                    } => {
                        // 原生确认对话框是阻塞调用，丢到 blocking 线程池别卡 async worker
                        // （接管态下通常不会走到这里，见 needs_user_attention）
                        let app_for_dialog = app.clone();
                        let confirmed = tauri::async_runtime::spawn_blocking(move || {
                            crate::tray::confirm_quit_chatgpt(&app_for_dialog, &target_name)
                        })
                        .await
                        .unwrap_or(false);
                        if !confirmed {
                            return Ok(());
                        }
                        user_choice = Some(true);
                    }
                    crate::commands::SwitchTierCommandResult::Switched { result } => {
                        for warning in &result.warnings {
                            log::warn!("[AutoMode] 切换档位模型后警告: {warning}");
                        }
                        return Ok(());
                    }
                }
            }
        }
    }

    // 刷新托盘菜单，确保勾选状态同步
    if let Ok(new_menu) = crate::tray::create_tray_menu(&app, state) {
        if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
            let _ = tray.set_menu(Some(new_menu));
        }
    }

    Ok(())
}

/// 设置某应用的省心选路模式（auto / manual）。
///
/// 首次切到 manual 且还没有手动清单时，把当前选路序快照成初始清单 ——
/// 用户从现状开始拖，不给空白列表。
#[tauri::command]
pub async fn set_easy_mode_mode(
    state: tauri::State<'_, AppState>,
    app_type: String,
    mode: String,
) -> Result<(), String> {
    require_auto_mode_app(&app_type)?;
    let parsed = auto_strategy::EasyModeMode::from_setting_value(&mode);
    if parsed.as_str() != mode {
        return Err(format!("未知的省心选路模式: {mode}"));
    }
    if parsed == auto_strategy::EasyModeMode::Manual
        && auto_strategy::get_manual_order(&state.db, &app_type).is_empty()
    {
        let snapshot: Vec<String> =
            auto_strategy::rank_managed_tier_candidates(&state.db, &app_type, false)
                .map_err(|e| e.to_string())?
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.id)
                .collect();
        if !snapshot.is_empty() {
            auto_strategy::set_manual_order(&state.db, &app_type, &snapshot)
                .map_err(|e| e.to_string())?;
        }
    }
    auto_strategy::set_mode(&state.db, &app_type, parsed).map_err(|e| e.to_string())
}

/// 写某应用的手动档位顺序（前端拖拽落定后整份提交）。
#[tauri::command]
pub async fn set_easy_mode_manual_order(
    state: tauri::State<'_, AppState>,
    app_type: String,
    ordered_ids: Vec<String>,
) -> Result<(), String> {
    require_auto_mode_app(&app_type)?;
    auto_strategy::set_manual_order(&state.db, &app_type, &ordered_ids).map_err(|e| e.to_string())
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
    /// 最近一次失败的上游报错原文（`ProxyError::to_string()`，成功即被清空）。
    /// 「为什么不选用」标签的数据源。
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
    require_auto_mode_app(app_type)?;
    let db = &state.db;
    let providers = db.get_all_providers(app_type).map_err(|e| e.to_string())?;
    // honor_affinity=false：看板展示纯策略序/手动序。会话亲和置顶是选路语义
    // （防中途换档丢提示词缓存），展示要的是「价格序 + 谁在用标当前 + 没在用
    // 的给出原因」的心智模型，两者别共用一个形状。
    let ranked = auto_strategy::rank_managed_tier_candidates(db, app_type, false)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
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
    let model_pref = auto_strategy::get_model_pref(db, app_type);
    let current_id = auto_strategy::effective_current_provider_id(db, app_type);

    let last_activity = db.get_provider_last_activity(app_type).unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let provider_ids: Vec<String> = ranked.iter().map(|p| p.id.clone()).collect();
    let breaker_states = state
        .proxy_service
        .provider_breaker_states(app_type, &provider_ids)
        .await;

    let balances = fetch_site_balances(app_type, &ranked).await;

    // 健康快照（缺行时 DAO 合成默认健康行，见 get_provider_health 的契约）
    let mut health: std::collections::HashMap<String, crate::proxy::types::ProviderHealth> =
        std::collections::HashMap::new();
    for p in &ranked {
        if let Ok(h) = db.get_provider_health(&p.id, app_type).await {
            health.insert(p.id.clone(), h);
        }
    }

    // 验真判定（两源读侧合并已在 store 层做完）：跨模型取最严重，只上异常
    let mut verification: std::collections::HashMap<
        String,
        crate::relay::model_verification::types::VerificationReport,
    > = std::collections::HashMap::new();
    for report in crate::relay::model_verification::store::list_for_provider_ids(db, &provider_ids)
        .unwrap_or_default()
    {
        match verification.get(&report.target.provider_id) {
            Some(current) => {
                if crate::relay::model_verification::verdict::report_precedes(&report, current) {
                    verification.insert(report.target.provider_id.clone(), report);
                }
            }
            None => {
                verification.insert(report.target.provider_id.clone(), report);
            }
        }
    }
    let verification_verdict = |provider_id: &str| -> Option<String> {
        verification
            .get(provider_id)
            .and_then(|report| match report.verdict {
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
        .map(|(position, p)| TierBoardTier {
            is_current: current_id.as_deref() == Some(p.id.as_str()),
            effective_model: model_pref
                .clone()
                .or_else(|| crate::relay::provision::extract_model(&p.settings_config)),
            unit_price_per_million: auto_strategy::effective_unit_price(
                db,
                &p,
                model_pref.as_deref(),
            ),
            rate_multiplier: multipliers.get(&p.id).copied(),
            avg_first_token_ms: ttft.get(&p.id).copied(),
            balance_usd: balances.get(&p.id).copied(),
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
            affinity_remaining_secs: (current_id.as_deref() == Some(p.id.as_str()))
                .then(|| {
                    last_activity
                        .get(&p.id)
                        .map(|last| {
                            (last + crate::proxy::auto_strategy::AFFINITY_WINDOW_SECS - now).max(0)
                                as u64
                        })
                        .filter(|remaining| *remaining > 0)
                })
                .flatten(),
            provider_id: p.id.clone(),
            name: p.name.clone(),
            position,
        })
        .collect();

    // 模型选项：全部托管档的目录并集（不按偏好过滤——选择器清单不因当前
    // 偏好缩水），每模型统计覆盖档数与最便宜「倍率×单价」（与排序同源；
    // 倍率或价格未知的档不参与最低价比较）
    let mut model_options: Vec<TierBoardModelOption> = Vec::new();
    for provider in providers.values() {
        if !crate::relay::is_managed(&provider.id) {
            continue;
        }
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
        mode: auto_strategy::get_mode(db, app_type).as_str().to_string(),
        strategy: auto_strategy::get_strategy(db).as_str().to_string(),
        model: model_pref,
        model_options,
        current_provider_id: current_id,
        tiers,
    })
}

/// 站点钱包余额：按「站点 origin」去重，每个 origin 用第一把 sk 查一次，回填到
/// 该站的全部档位上。两路按序试：先 sub2api 的 `GET /v1/usage`（sub2api 站短路，
/// 不多花请求）；问不出（典型 newapi：该端点 404）再回落 one-api 系 billing 双端
/// 点（见 [`crate::relay::api::billing_balance_with_api_key`] 的口径与限制）。
/// 两路都拿不到 → 该站各档 `None`。只对 https 端点发起（http/本地/无 sk 自然
/// 跳过，单元测试零网络）。
async fn fetch_site_balances(
    app_type: &str,
    tiers: &[crate::provider::Provider],
) -> std::collections::HashMap<String, f64> {
    use crate::app_config::AppType;
    let app = match AppType::from_str(app_type) {
        Ok(app) => app,
        Err(_) => return std::collections::HashMap::new(),
    };
    let Some(adapter) = crate::proxy::providers::get_adapter(&app) else {
        return std::collections::HashMap::new();
    };

    let mut tier_origin: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut origin_key: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for tier in tiers {
        let (base, auth) = match (adapter.extract_base_url(tier), adapter.extract_auth(tier)) {
            (Ok(base), Some(auth)) => (base, auth),
            _ => continue,
        };
        let Some(origin) = origin_of(&base) else {
            continue;
        };
        if !origin.starts_with("https://") {
            continue;
        }
        tier_origin.insert(tier.id.clone(), origin.clone());
        origin_key.entry(origin).or_insert(auth.api_key);
    }

    let mut balances = std::collections::HashMap::new();
    let queries: Vec<_> = origin_key
        .into_iter()
        .map(|(origin, key)| async move {
            let sub2api_wallet = crate::relay::api::usage_with_api_key(&origin, &key)
                .await
                .ok()
                .and_then(|usage| {
                    usage
                        .data
                        .and_then(|items| items.first().and_then(|item| item.remaining))
                });
            let balance = match sub2api_wallet {
                Some(balance) => Some(balance),
                None => crate::relay::api::billing_balance_with_api_key(&origin, &key)
                    .await
                    .ok()
                    .flatten(),
            };
            (origin, balance)
        })
        .collect();
    for (origin, balance) in futures::future::join_all(queries).await {
        if let Some(balance) = balance {
            for (tier_id, tier_origin) in &tier_origin {
                if tier_origin == &origin {
                    balances.insert(tier_id.clone(), balance);
                }
            }
        }
    }
    balances
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

/// 从 base_url 取 `scheme://authority`（`https://site/v1` → `https://site`）。
fn origin_of(base_url: &str) -> Option<String> {
    let (scheme, rest) = base_url.split_once("://")?;
    let authority = rest.split('/').next()?;
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
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

        let board = tier_board_impl(&state, "claude").await.unwrap();
        assert_eq!(board.mode, "auto");
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

        // 手动模式：手动序反映到看板
        crate::proxy::auto_strategy::set_mode(
            &db,
            "claude",
            crate::proxy::auto_strategy::EasyModeMode::Manual,
        )
        .unwrap();
        crate::proxy::auto_strategy::set_manual_order(
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
    async fn tier_board_stays_pure_price_order_with_active_current() {
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
    async fn tier_board_surfaces_affinity_countdown_for_active_current() {
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
        let remaining = by_id(&current)
            .affinity_remaining_secs
            .expect("活跃当前档位必须有亲和倒计时");
        assert!(
            remaining > 0 && remaining <= 30 * 60,
            "剩余 {remaining} 应在 (0, 30min]"
        );
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

    /// 亲和判据与选路同源（proxy_request_logs 近期流量），见
    /// `auto_strategy::AFFINITY_WINDOW_SECS`。
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
