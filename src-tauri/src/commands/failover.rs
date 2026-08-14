//! 故障转移队列命令
//!
//! 管理代理模式下的故障转移队列（基于 providers 表的 in_failover_queue 字段）

use crate::database::FailoverQueueItem;
use crate::events::PROVIDER_SWITCHED;
use crate::provider::Provider;
use crate::store::AppState;
use std::str::FromStr;
use tauri::Emitter;

fn require_failover_app(app_type: &str) -> Result<(), String> {
    let app = crate::app_config::AppType::from_str(app_type)
        .map_err(|error| format!("无效的应用类型: {error}"))?;
    if !app.supports_local_proxy() {
        return Err(format!("{} 不支持故障转移", app.as_str()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::require_failover_app;

    #[test]
    fn failover_rejects_apps_without_a_proxy_data_plane() {
        assert!(require_failover_app("claude").is_ok());
        assert!(require_failover_app("pi").is_err());
    }
}

/// 获取故障转移队列
#[tauri::command]
pub async fn get_failover_queue(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<Vec<FailoverQueueItem>, String> {
    require_failover_app(&app_type)?;
    state
        .db
        .get_failover_queue(&app_type)
        .map_err(|e| e.to_string())
}

/// 获取可添加到故障转移队列的供应商（不在队列中的）
#[tauri::command]
pub async fn get_available_providers_for_failover(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<Vec<Provider>, String> {
    require_failover_app(&app_type)?;
    let available = state
        .db
        .get_available_providers_for_failover(&app_type)
        .map_err(|e| e.to_string())?;

    // 托管档位不该出现在选择器里 —— 见 `add_to_failover_queue` 的守卫说明。
    // 这里滤掉是为了**不把拦得住的东西摆出来给人点**（点了会被守卫拒，但那是个坏体验）。
    Ok(available
        .into_iter()
        .filter(|p| !crate::relay::is_managed(&p.id))
        .collect())
}

/// 添加供应商到故障转移队列
#[tauri::command]
pub async fn add_to_failover_queue(
    state: tauri::State<'_, AppState>,
    app_type: String,
    provider_id: String,
) -> Result<(), String> {
    require_failover_app(&app_type)?;
    // 托管档位不许进队列。**这是队列这条链的唯一准入口，所以守卫只需要加在这里** ——
    // 下游三个消费点（开故障转移开关时切 P1、托盘 Auto、熔断自动切）切的都是队列里的
    // provider_id，队列里没有托管项，那三条路就不可能指向托管项。
    //
    // 为什么必须拦：熔断自动切是**用户没点任何按钮**就发生的（FailoverSwitchManager
    // 检测到上游报错后自己切），一旦切到托管档位，就跳过了「退出 ChatGPT → 切换 → 重开」
    // 的编排 —— codex 的 live 配置被换成托管 sk 而 ChatGPT 还连着旧的，用户全程无感。
    // 托盘菜单过滤（tray.rs 的 filter_unmanaged）在这条路上完全无效，因为切换不是从
    // 菜单点出来的。
    crate::relay::reject_if_managed(&provider_id).map_err(|e| e.to_string())?;

    state
        .db
        .add_to_failover_queue(&app_type, &provider_id)
        .map_err(|e| e.to_string())
}

/// 从故障转移队列移除供应商
#[tauri::command]
pub async fn remove_from_failover_queue(
    state: tauri::State<'_, AppState>,
    app_type: String,
    provider_id: String,
) -> Result<(), String> {
    require_failover_app(&app_type)?;
    state
        .db
        .remove_from_failover_queue(&app_type, &provider_id)
        .map_err(|e| e.to_string())
}

/// 获取指定应用的自动故障转移开关状态（从 proxy_config 表读取）
#[tauri::command]
pub async fn get_auto_failover_enabled(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<bool, String> {
    require_failover_app(&app_type)?;
    state
        .db
        .get_proxy_config_for_app(&app_type)
        .await
        .map(|config| config.auto_failover_enabled)
        .map_err(|e| e.to_string())
}

/// 设置指定应用的自动故障转移开关状态（写入 proxy_config 表）
///
/// 注意：关闭故障转移时不会清除队列，队列内容会保留供下次开启时使用
#[tauri::command]
pub async fn set_auto_failover_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_type: String,
    enabled: bool,
) -> Result<(), String> {
    require_failover_app(&app_type)?;
    log::info!(
        "[Failover] Setting auto_failover_enabled: app_type='{app_type}', enabled={enabled}"
    );

    // 读取当前配置
    let mut config = state
        .db
        .get_proxy_config_for_app(&app_type)
        .await
        .map_err(|e| e.to_string())?;

    if enabled && !config.enabled {
        return Err("需要先启用该应用的代理接管，再开启故障转移".to_string());
    }

    // 队列为空时把当前供应商自动加入作为 P1，避免用户陷入"必须先加队列才能开启"的死锁
    let mut auto_added_provider_id: Option<String> = None;
    let p1_provider_id = if enabled {
        let mut queue = state
            .db
            .get_failover_queue(&app_type)
            .map_err(|e| e.to_string())?;

        if queue.is_empty() {
            let app_enum = crate::app_config::AppType::from_str(&app_type)
                .map_err(|_| format!("无效的应用类型: {app_type}"))?;

            let current_id = crate::settings::get_effective_current_provider(&state.db, &app_enum)
                .map_err(|e| e.to_string())?;

            let Some(current_id) = current_id else {
                return Err("故障转移队列为空，且未设置当前供应商，无法开启故障转移".to_string());
            };

            // 这里是队列的**第二个准入口**，且它绕过了 `add_to_failover_queue` 命令
            // （直接调 `state.db`），所以那道守卫在这里不生效，必须再拦一次。
            //
            // 真会走到：用户当前正用着某个托管档位、队列还空着，此时开故障转移开关 ⇒
            // 「自动把当前 provider 作为 P1 加入」就把托管档位塞进了队列，
            // 之后每次熔断都会自动切到它。
            //
            // 拦下而不是「跳过自动添加」：跳过的结果是队列仍为空、下面 `queue.first()`
            // 拿不到 P1 而报一句语焉不详的「队列为空」，用户不知道为什么。
            if crate::relay::is_managed(&current_id) {
                return Err("当前用的是 LoongPort 托管的档位，它不能作为故障转移目标。\
                            请先切到普通供应商，或手动往队列里加至少一个供应商，再开启故障转移。"
                    .to_string());
            }

            state
                .db
                .add_to_failover_queue(&app_type, &current_id)
                .map_err(|e| e.to_string())?;
            auto_added_provider_id = Some(current_id);

            queue = state
                .db
                .get_failover_queue(&app_type)
                .map_err(|e| e.to_string())?;
        }

        queue
            .first()
            .map(|item| item.provider_id.clone())
            .ok_or_else(|| "故障转移队列为空，无法开启故障转移".to_string())?
    } else {
        String::new()
    };

    // 开启前先切到 P1。只有切换成功后才写入 auto_failover_enabled=true，
    // 避免 P1 不可切换（例如 official provider）时留下“开关已开但目标未切”的脏状态。
    if enabled {
        if let Err(e) = state
            .proxy_service
            .switch_proxy_target(&app_type, &p1_provider_id)
            .await
        {
            if let Some(provider_id) = auto_added_provider_id {
                let _ = state.db.remove_from_failover_queue(&app_type, &provider_id);
            }
            return Err(e);
        }
    }

    // 更新 auto_failover_enabled 字段
    config.auto_failover_enabled = enabled;

    // 写回数据库
    state
        .db
        .update_proxy_config_for_app(config)
        .await
        .map_err(|e| e.to_string())?;

    if enabled {
        // 发射 provider-switched 事件（让前端刷新当前供应商）
        let event_data = serde_json::json!({
            "appType": app_type,
            "providerId": p1_provider_id,
            "source": "failoverEnabled"
        });
        let _ = app.emit(PROVIDER_SWITCHED, event_data);
    }

    // 刷新托盘菜单，确保状态同步
    if let Ok(new_menu) = crate::tray::create_tray_menu(&app, &state) {
        if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
            let _ = tray.set_menu(Some(new_menu));
        }
    }

    Ok(())
}
