//! 故障转移切换模块
//!
//! 处理故障转移成功后的供应商切换逻辑，包括：
//! - 去重控制（避免多个请求同时触发）
//! - 托盘菜单更新
//! - 前端事件发射

use crate::database::Database;
use crate::error::AppError;
#[cfg(feature = "gui")]
use crate::events::PROVIDER_SWITCHED;
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(feature = "gui")]
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;

/// 故障转移切换管理器
///
/// 负责处理故障转移成功后的供应商切换，确保 UI 能够直观反映当前使用的供应商。
#[derive(Clone)]
pub struct FailoverSwitchManager {
    /// 正在处理中的切换（key = "app_type:provider_id"）
    pending_switches: Arc<RwLock<HashSet<String>>>,
    db: Arc<Database>,
    service_owner: std::sync::Weak<crate::services::ProxyService>,
}

impl FailoverSwitchManager {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            pending_switches: Arc::new(RwLock::new(HashSet::new())),
            db,
            service_owner: std::sync::Weak::new(),
        }
    }

    pub(crate) fn set_service_owner(
        &mut self,
        owner: std::sync::Weak<crate::services::ProxyService>,
    ) {
        self.service_owner = owner;
    }

    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pending_switches, &other.pending_switches)
    }

    /// 尝试执行故障转移切换
    ///
    /// 如果相同的切换已在进行中，则跳过；否则执行切换逻辑。
    ///
    /// # Returns
    /// - `Ok(true)` - 切换成功执行
    /// - `Ok(false)` - 切换已在进行中，跳过
    /// - `Err(e)` - 切换过程中发生错误
    pub async fn try_switch(
        &self,
        #[cfg(feature = "gui")] app_handle: Option<&tauri::AppHandle>,
        app_type: &str,
        provider_id: &str,
        provider_name: &str,
        expected_current: &str,
        request_identity: Option<&crate::services::proxy::RequestIdentity>,
    ) -> Result<bool, AppError> {
        let switch_key = format!("{app_type}:{provider_id}");

        // 去重检查：如果相同切换已在进行中，跳过
        {
            let mut pending = self.pending_switches.write().await;
            if pending.contains(&switch_key) {
                log::debug!("[Failover] 切换已在进行中，跳过: {app_type} -> {provider_id}");
                return Ok(false);
            }
            pending.insert(switch_key.clone());
        }

        // 执行切换（确保最后清理 pending 标记）
        let result = self
            .do_switch(
                #[cfg(feature = "gui")]
                app_handle,
                app_type,
                provider_id,
                provider_name,
                expected_current,
                request_identity,
            )
            .await;

        // 清理 pending 标记
        {
            let mut pending = self.pending_switches.write().await;
            pending.remove(&switch_key);
        }

        result
    }

    async fn do_switch(
        &self,
        #[cfg(feature = "gui")] app_handle: Option<&tauri::AppHandle>,
        app_type: &str,
        provider_id: &str,
        provider_name: &str,
        expected_current: &str,
        request_identity: Option<&crate::services::proxy::RequestIdentity>,
    ) -> Result<bool, AppError> {
        // 检查该应用是否已被代理接管（enabled=true）
        // 只有被接管的应用才允许执行故障转移切换
        let app_enabled = match self.db.get_proxy_config_for_app(app_type).await {
            Ok(config) => config.enabled && config.auto_failover_enabled,
            Err(e) => {
                log::warn!("[FO-002] 无法读取 {app_type} 配置: {e}，跳过切换");
                return Ok(false);
            }
        };

        if !app_enabled {
            log::debug!("[Failover] {app_type} 未启用代理，跳过切换");
            return Ok(false);
        }

        log::info!("[FO-001] 切换: {app_type} → {provider_name}");

        let Some(service) = self.service_owner.upgrade() else {
            return Err(AppError::Config("proxy.owner_unavailable".into()));
        };
        let Some(guard) = service.lock_active_failover(app_type, self).await else {
            return Ok(false);
        };
        if crate::mode::operation::uses_upstream4_schema(&self.db)? {
            if !service.request_identity_is_current(app_type, request_identity)? {
                return Ok(false);
            }
            let app: crate::app_config::AppType = app_type.parse()?;
            let mode = {
                let vault = self.db.secret_session().read()?;
                crate::mode::current::validate_known_mode(
                    &crate::live::engine::DeviceStore::for_device(),
                    &vault,
                    &app,
                )?
            };
            if !mode.is_proxy() || !mode.attached || !service.is_running().await {
                return Ok(false);
            }
        }
        let current = super::application_routing::current_provider_id_checked(&self.db, app_type)?;
        if current.as_deref().unwrap_or_default() != expected_current {
            return Ok(false);
        }
        let chain = super::application_routing::chain_providers(&self.db, app_type)?;
        if !chain.iter().any(|p| p.id == provider_id)
            || !super::application_routing::failover_enabled(&self.db, app_type)?
        {
            return Ok(false);
        }
        let switched = service
            .hot_switch_provider_inner(app_type, provider_id)
            .await
            .map_err(AppError::Message)?
            .logical_target_changed;
        drop(guard);
        #[cfg(feature = "gui")]
        if switched {
            if let Some(app) = app_handle {
                if let Some(app_state) = app.try_state::<crate::store::AppState>() {
                    if let Ok(menu) = crate::tray::create_tray_menu(app, app_state.inner()) {
                        if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
                            if let Err(error) = tray.set_menu(Some(menu)) {
                                log::error!("[Failover] 更新托盘菜单失败: {error}");
                            }
                        }
                    }
                }
                if let Err(error) = app.emit(PROVIDER_SWITCHED, serde_json::json!({"appType": app_type, "providerId": provider_id, "source": "failover"})) { log::error!("[Failover] 发射事件失败: {error}"); }
            }
        }

        Ok(switched)
    }
}
