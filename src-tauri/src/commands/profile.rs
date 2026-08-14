//! 项目 Profile 管理命令

use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::database::Profile;
use crate::events::{PROFILE_APPLIED, PROVIDER_SWITCHED};
use crate::services::profile::{ProfilePayload, ProfileScope, ProfileService};
use crate::store::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDto {
    pub id: String,
    pub name: String,
    pub payload: ProfilePayload,
    pub scope_snapshots: Vec<ProfileScopeSnapshotDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

impl From<Profile> for ProfileDto {
    fn from(profile: Profile) -> Self {
        // 单条 payload 损坏不应拖垮整个列表：降级为默认值并记日志
        let payload = serde_json::from_str(&profile.payload).unwrap_or_else(|e| {
            log::warn!(
                "解析 profile '{}' payload 失败，使用默认值: {e}",
                profile.id
            );
            ProfilePayload::default()
        });
        Self {
            id: profile.id,
            name: profile.name,
            scope_snapshots: ProfileScope::ALL
                .into_iter()
                .map(|scope| ProfileScopeSnapshotDto {
                    scope,
                    has_snapshot: payload.scope_captured(scope),
                })
                .collect(),
            payload,
            created_at: profile.created_at,
            updated_at: profile.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileScopeSnapshotDto {
    pub scope: ProfileScope,
    pub has_snapshot: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAppDto {
    pub app: AppType,
    pub supported: bool,
    pub scope: Option<ProfileScope>,
    pub current_profile_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesResponse {
    pub profiles: Vec<ProfileDto>,
    pub apps: Vec<ProfileAppDto>,
}

fn build_profile_app_dtos<E>(
    mut current_profile_id: impl FnMut(ProfileScope) -> Result<Option<String>, E>,
) -> Result<Vec<ProfileAppDto>, E> {
    AppType::all()
        .map(|app| {
            let scope = ProfileScope::for_app(&app);
            Ok(ProfileAppDto {
                app,
                supported: scope.is_some(),
                scope,
                current_profile_id: match scope {
                    Some(scope) => current_profile_id(scope)?,
                    None => None,
                },
            })
        })
        .collect()
}

/// Profile 应用完成后的统一收尾：发事件 + 重建托盘菜单
///
/// 只对项目所属分组内的应用发 provider-switched。UI 与托盘两个入口必须
/// 共用此函数，保证事件 payload 形状一致（前端 App.tsx 的
/// provider-switched 监听依赖该形状）。
pub fn emit_profile_apply_events(
    app: &tauri::AppHandle,
    state: &AppState,
    profile_id: &str,
    scope: ProfileScope,
) {
    for app_type in scope.apps().iter() {
        let app_str = app_type.as_str();
        let (proxy_enabled, auto_failover_enabled) = state.db.get_proxy_flags_sync(app_str);
        let provider_id = crate::settings::get_effective_current_provider(&state.db, app_type)
            .ok()
            .flatten()
            .unwrap_or_default();
        let event_data = serde_json::json!({
            "appType": app_str,
            "proxyEnabled": proxy_enabled,
            "autoFailoverEnabled": auto_failover_enabled,
            "providerId": provider_id,
        });
        if let Err(e) = app.emit(PROVIDER_SWITCHED, event_data) {
            log::error!("发射 {PROVIDER_SWITCHED} 事件失败: {e}");
        }
    }
    if let Err(e) = app.emit(
        PROFILE_APPLIED,
        serde_json::json!({ "profileId": profile_id, "scope": scope.as_str() }),
    ) {
        log::error!("发射 {PROFILE_APPLIED} 事件失败: {e}");
    }
    crate::tray::refresh_tray_menu(app);
}

#[tauri::command]
pub fn list_profiles(state: State<'_, AppState>) -> Result<ProfilesResponse, String> {
    let profiles = ProfileService::list(&state).map_err(|e| e.to_string())?;
    let apps = build_profile_app_dtos(|scope| {
        state
            .db
            .get_current_profile_id(scope.as_str())
            .map_err(|e| e.to_string())
    })?;
    Ok(ProfilesResponse {
        profiles: profiles.into_iter().map(ProfileDto::from).collect(),
        apps,
    })
}

#[tauri::command]
pub fn create_profile(
    state: State<'_, AppState>,
    name: String,
    scope: String,
) -> Result<ProfileDto, String> {
    let scope = ProfileScope::parse(&scope).map_err(|e| e.to_string())?;
    ProfileService::create(&state, &name, scope)
        .map(ProfileDto::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_profile(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    resnapshot: Option<bool>,
    scope: Option<String>,
) -> Result<ProfileDto, String> {
    let scope = scope
        .map(|s| ProfileScope::parse(&s))
        .transpose()
        .map_err(|e| e.to_string())?;
    ProfileService::update(&state, &id, name, resnapshot.unwrap_or(false), scope)
        .map(ProfileDto::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_profile(state: State<'_, AppState>, id: String) -> Result<(), String> {
    ProfileService::delete(&state, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_current_profile(state: State<'_, AppState>, scope: String) -> Result<(), String> {
    let scope = ProfileScope::parse(&scope).map_err(|e| e.to_string())?;
    state
        .db
        .set_current_profile_id(scope.as_str(), None)
        .map_err(|e| e.to_string())
}

/// 应用项目快照（只作用于发起页所属分组内的应用）。
///
/// 注意：必须保持同步命令（跑在 Tauri 线程池）——`ProviderService::switch`
/// 内部使用 block_on 获取切换锁，放进 async 命令会在运行时线程上 panic。
#[tauri::command]
pub fn apply_profile(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    scope: String,
) -> Result<Vec<String>, String> {
    let scope = ProfileScope::parse(&scope).map_err(|e| e.to_string())?;
    let (warnings, should_stop_proxy) =
        ProfileService::apply(&state, &id, scope).map_err(|e| e.to_string())?;

    if should_stop_proxy {
        // sync 命令线程没有 Tokio runtime，无法直接 await stop()；
        // 把停止服务放到 Tauri async runtime，停止后再补发事件刷新 UI。
        let app_handle = app.clone();
        let profile_id = id.clone();
        let proxy_service = state.proxy_service.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = proxy_service.stop().await {
                log::warn!("切换项目后停止代理服务失败: {e}");
            }
            if let Some(app_state) = app_handle.try_state::<AppState>() {
                emit_profile_apply_events(&app_handle, app_state.inner(), &profile_id, scope);
            }
        });
    } else {
        emit_profile_apply_events(&app, &state, &id, scope);
    }

    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with_payload(payload: ProfilePayload) -> Profile {
        Profile {
            id: "profile-1".to_string(),
            name: "Project One".to_string(),
            payload: serde_json::to_string(&payload).unwrap(),
            sort_order: None,
            created_at: Some(1),
            updated_at: Some(2),
        }
    }

    #[test]
    fn profile_dto_exposes_snapshot_presence_for_each_backend_scope() {
        let mut payload = ProfilePayload::default();
        payload.mcp.claude = Some(vec![]);
        payload.providers.codex = Some("codex-provider".to_string());

        let value = serde_json::to_value(ProfileDto::from(profile_with_payload(payload))).unwrap();

        assert_eq!(
            value["scopeSnapshots"],
            serde_json::json!([
                { "scope": "claude", "hasSnapshot": true },
                { "scope": "claude-desktop", "hasSnapshot": false },
                { "scope": "codex", "hasSnapshot": true }
            ])
        );
    }

    #[test]
    fn profile_app_dtos_expose_backend_support_scope_and_current_profile() {
        let apps = build_profile_app_dtos(|scope| {
            Ok::<_, String>(match scope {
                ProfileScope::Claude => Some("claude-current".to_string()),
                ProfileScope::ClaudeDesktop => None,
                ProfileScope::Codex => Some("codex-current".to_string()),
            })
        })
        .unwrap();

        assert!(apps.iter().any(|app| !app.supported && app.scope.is_none()));

        let value = serde_json::to_value(apps).unwrap();
        let apps = value.as_array().unwrap();
        assert_eq!(
            apps.iter().find(|app| app["app"] == "codex").unwrap(),
            &serde_json::json!({
                "app": "codex",
                "supported": true,
                "scope": "codex",
                "currentProfileId": "codex-current"
            })
        );
        assert_eq!(
            apps.iter().find(|app| app["app"] == "codex-image").unwrap(),
            &serde_json::json!({
                "app": "codex-image",
                "supported": false,
                "scope": null,
                "currentProfileId": null
            })
        );
    }
}
