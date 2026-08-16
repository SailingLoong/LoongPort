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

#[cfg(test)]
mod tests {
    use super::require_auto_mode_app;

    #[test]
    fn auto_mode_rejects_apps_without_a_proxy_data_plane() {
        assert!(require_auto_mode_app("claude").is_ok());
        assert!(require_auto_mode_app("pi").is_err());
    }
}
