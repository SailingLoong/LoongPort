use indexmap::IndexMap;
use std::collections::HashMap;

use crate::app_config::{AppType, McpServer};
use crate::error::AppError;
use crate::mcp;
use crate::store::AppState;

/// MCP 相关业务逻辑（v3.7.0 统一结构）
pub struct McpService;

impl McpService {
    /// 获取所有 MCP 服务器（统一结构）
    pub fn get_all_servers(state: &AppState) -> Result<IndexMap<String, McpServer>, AppError> {
        state.db.get_all_mcp_servers()
    }

    /// 添加或更新 MCP 服务器
    pub fn upsert_server(state: &AppState, server: McpServer) -> Result<(), AppError> {
        // 读取旧状态：用于处理“编辑时取消勾选某个应用”的场景（需要从对应 live 配置中移除）
        let prev_apps = state
            .db
            .get_all_mcp_servers()?
            .get(&server.id)
            .map(|s| s.apps.clone())
            .unwrap_or_default();

        state.db.save_mcp_server(&server)?;

        // 处理禁用：若旧版本启用但新版本取消，则需要从该应用的 live 配置移除
        if prev_apps.claude && !server.apps.claude {
            Self::remove_server_from_app(state, &server.id, &AppType::Claude)?;
        }
        if prev_apps.codex && !server.apps.codex {
            Self::remove_server_from_app(state, &server.id, &AppType::Codex)?;
        }
        if prev_apps.gemini && !server.apps.gemini {
            Self::remove_server_from_app(state, &server.id, &AppType::Gemini)?;
        }
        if prev_apps.grokbuild && !server.apps.grokbuild {
            Self::remove_server_from_app(state, &server.id, &AppType::GrokBuild)?;
        }
        if prev_apps.opencode && !server.apps.opencode {
            Self::remove_server_from_app(state, &server.id, &AppType::OpenCode)?;
        }
        if prev_apps.hermes && !server.apps.hermes {
            Self::remove_server_from_app(state, &server.id, &AppType::Hermes)?;
        }

        // 同步到各个启用的应用
        Self::sync_server_to_apps(state, &server)?;

        Ok(())
    }

    /// 删除 MCP 服务器
    pub fn delete_server(state: &AppState, id: &str) -> Result<bool, AppError> {
        let server = state.db.get_all_mcp_servers()?.shift_remove(id);

        if let Some(server) = server {
            state.db.delete_mcp_server(id)?;

            // 从所有应用的 live 配置中移除
            Self::remove_server_from_all_apps(state, id, &server)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 命令入口：与供应商切换共用 per-App 锁，保护整个 live 配置读改写。
    ///
    /// 已持有该锁的内部调用方直接使用 `toggle_app`，避免不可重入的锁嵌套。
    pub async fn toggle_app_locked(
        state: &AppState,
        server_id: &str,
        app: AppType,
        enabled: bool,
    ) -> Result<(), AppError> {
        let _guard = state.proxy_service.lock_switch_for_app(app.as_str()).await;
        Self::toggle_app(state, server_id, app, enabled)
    }

    /// 切换指定应用的启用状态；调用方负责持有该应用的切换锁。
    pub fn toggle_app(
        state: &AppState,
        server_id: &str,
        app: AppType,
        enabled: bool,
    ) -> Result<(), AppError> {
        if let Some(server) = state
            .db
            .update_mcp_server_app_enabled(server_id, &app, enabled)?
        {
            // 同步到对应应用
            if enabled {
                Self::sync_server_to_app(state, &server, &app)?;
            } else {
                Self::remove_server_from_app(state, server_id, &app)?;
            }
        }

        Ok(())
    }

    /// 将 MCP 服务器同步到所有启用的应用
    fn sync_server_to_apps(state: &AppState, server: &McpServer) -> Result<(), AppError> {
        let mut failures = Vec::new();
        for app in server.apps.enabled_apps() {
            if let Err(err) = Self::sync_server_to_app_no_config(state, server, &app) {
                log::warn!("同步 MCP '{}' 到 {app:?} 失败: {err}", server.id);
                failures.push(format!("{}: {err}", app.as_str()));
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "部分应用 MCP 同步失败: {}",
                failures.join("; ")
            )))
        }
    }

    /// 将 MCP 服务器同步到指定应用
    fn sync_server_to_app(
        state: &AppState,
        server: &McpServer,
        app: &AppType,
    ) -> Result<(), AppError> {
        Self::sync_server_to_app_no_config(state, server, app)
    }

    fn sync_server_to_app_no_config(
        state: &AppState,
        server: &McpServer,
        app: &AppType,
    ) -> Result<(), AppError> {
        match app {
            AppType::Claude => {
                mcp::sync_single_server_to_claude(&Default::default(), &server.id, &server.server)?;
            }
            AppType::ClaudeDesktop => {
                log::debug!("Claude Desktop 3P profiles do not use CC Switch MCP sync, skipping");
            }
            AppType::CodexImage => {
                // 生图栏本身不是一个装 MCP 的 CLI —— 生图工具装进 codex/claude/gemini。
                log::debug!("生图栏不同步 MCP，跳过");
            }
            AppType::Codex => {
                // Codex uses TOML format, must use the correct function
                mcp::sync_single_server_to_codex(&Default::default(), &server.id, &server.server)?;
            }
            AppType::Gemini => {
                mcp::sync_single_server_to_gemini(&Default::default(), &server.id, &server.server)?;
            }
            AppType::GrokBuild => {
                mcp::sync_single_server_to_grokbuild(
                    &Default::default(),
                    &server.id,
                    &server.server,
                )?;
            }
            AppType::OpenCode => {
                mcp::sync_single_server_to_opencode(
                    &Default::default(),
                    &server.id,
                    &server.server,
                )?;
            }
            AppType::OpenClaw => {
                // OpenClaw MCP support is still in development (Issue #4834)
                // Skip for now
                log::debug!("OpenClaw MCP support is still in development, skipping sync");
            }
            AppType::Hermes => {
                mcp::sync_single_server_to_hermes(
                    state.db.secret_session(),
                    &Default::default(),
                    &server.id,
                    &server.server,
                )?;
            }
            AppType::Pi | AppType::Mcode => {}
        }
        Ok(())
    }

    /// 从所有曾启用过该服务器的应用中移除
    fn remove_server_from_all_apps(
        state: &AppState,
        id: &str,
        server: &McpServer,
    ) -> Result<(), AppError> {
        // 从所有曾启用的应用中移除
        for app in server.apps.enabled_apps() {
            Self::remove_server_from_app(state, id, &app)?;
        }
        Ok(())
    }

    fn remove_server_from_app(state: &AppState, id: &str, app: &AppType) -> Result<(), AppError> {
        match app {
            AppType::Claude => mcp::remove_server_from_claude(id)?,
            AppType::ClaudeDesktop => {
                log::debug!("Claude Desktop 3P profiles do not use CC Switch MCP sync, skipping");
            }
            AppType::CodexImage => {
                log::debug!("生图栏不同步 MCP，跳过");
            }
            AppType::Codex => mcp::remove_server_from_codex(id)?,
            AppType::Gemini => mcp::remove_server_from_gemini(id)?,
            AppType::GrokBuild => mcp::remove_server_from_grokbuild(id)?,
            AppType::OpenCode => {
                mcp::remove_server_from_opencode(id)?;
            }
            AppType::OpenClaw => {
                // OpenClaw MCP support is still in development
                log::debug!("OpenClaw MCP support is still in development, skipping remove");
            }
            AppType::Hermes => {
                mcp::remove_server_from_hermes(state.db.secret_session(), id)?;
            }
            AppType::Pi | AppType::Mcode => {}
        }
        Ok(())
    }

    /// 手动同步所有启用的 MCP 服务器到对应的应用。
    ///
    /// Best-effort：单个应用投影失败（如 ~/.claude.json 坏 JSON）不阻断
    /// 其余应用——各应用的 live 文件互相独立，一处损坏没有理由让其他
    /// 应用的 MCP 状态陈旧。全部跑完后若有失败，聚合成一个错误上报，
    /// 保留调用方的可见性。
    pub fn sync_all_enabled(state: &AppState) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;

        let mut failures: Vec<String> = Vec::new();
        for app in AppType::all() {
            if let Err(err) = Self::project_servers_to_app(state, &servers, &app) {
                log::warn!("同步 MCP 到 {app:?} 失败: {err}");
                failures.push(format!("{}: {err}", app.as_str()));
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "部分应用 MCP 同步失败: {}",
                failures.join("; ")
            )))
        }
    }

    /// 只把启用状态投影到单个应用。某个应用的 live 被整体重写后用它做
    /// 定向重投影，避免把无关应用的失败面（如 ~/.claude.json 坏 JSON）
    /// 牵连进目标应用的关键路径。
    pub fn sync_enabled_for_app(state: &AppState, app: &AppType) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;
        Self::project_servers_to_app(state, &servers, app)
    }

    fn project_servers_to_app(
        state: &AppState,
        servers: &IndexMap<String, McpServer>,
        app: &AppType,
    ) -> Result<(), AppError> {
        if matches!(
            app,
            AppType::OpenClaw | AppType::ClaudeDesktop | AppType::Pi | AppType::Mcode
        ) {
            return Ok(());
        }

        // 逐条容错：一条写不进去（规范不是对象、这个应用不认的类型）不该让排在
        // 它后面的服务器全都停在旧状态。全部跑完后聚合上报，调用方照旧能看到
        // 失败。错误相同的条目归并成一项：live 文件本身损坏时每一条都报同一句。
        let mut failures: IndexMap<String, Vec<&str>> = IndexMap::new();
        for server in servers.values() {
            let result = if server.apps.is_enabled_for(app) {
                Self::sync_server_to_app(state, server, app)
            } else {
                Self::remove_server_from_app(state, &server.id, app)
            };
            if let Err(err) = result {
                log::warn!("同步 MCP 服务器 '{}' 到 {app:?} 失败: {err}", server.id);
                failures
                    .entry(err.to_string())
                    .or_default()
                    .push(&server.id);
            }
        }

        if failures.is_empty() {
            return Ok(());
        }
        let detail = failures
            .iter()
            .map(|(error, ids)| format!("{}: {error}", ids.join(", ")))
            .collect::<Vec<_>>()
            .join("; ");
        Err(AppError::Message(format!(
            "部分 MCP 服务器同步到 {} 失败: {detail}",
            app.as_str()
        )))
    }

    // ========================================================================
    // 兼容层：支持旧的 v3.6.x 命令（已废弃，将在 v4.0 移除）
    // ========================================================================

    /// [已废弃] 获取指定应用的 MCP 服务器（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use get_all_servers instead")]
    pub fn get_servers(
        state: &AppState,
        app: AppType,
    ) -> Result<HashMap<String, serde_json::Value>, AppError> {
        let all_servers = Self::get_all_servers(state)?;
        let mut result = HashMap::new();

        for (id, server) in all_servers {
            if server.apps.is_enabled_for(&app) {
                result.insert(id, server.server);
            }
        }

        Ok(result)
    }

    /// [已废弃] 设置 MCP 服务器在指定应用的启用状态（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use toggle_app instead")]
    pub fn set_enabled(
        state: &AppState,
        app: AppType,
        id: &str,
        enabled: bool,
    ) -> Result<bool, AppError> {
        Self::toggle_app(state, id, app, enabled)?;
        Ok(true)
    }

    /// [已废弃] 同步启用的 MCP 到指定应用（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use sync_all_enabled instead")]
    pub fn sync_enabled(state: &AppState, app: AppType) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;

        for server in servers.values() {
            if server.apps.is_enabled_for(&app) {
                Self::sync_server_to_app(state, server, &app)?;
            }
        }

        Ok(())
    }

    /// 从 Claude 导入 MCP，不反向写回原生配置。
    pub fn import_from_claude(state: &AppState) -> Result<usize, AppError> {
        let mut temp_config = crate::app_config::MultiAppConfig::default();
        let count = crate::mcp::import_from_claude(&mut temp_config)?;
        if count == 0 {
            return Ok(0);
        }
        Self::persist_imported_servers(state, temp_config.mcp.servers.as_ref(), &AppType::Claude)
    }

    /// 从 Codex 导入 MCP，不反向写回原生配置。
    pub fn import_from_codex(state: &AppState) -> Result<usize, AppError> {
        let mut temp_config = crate::app_config::MultiAppConfig::default();
        let count = crate::mcp::import_from_codex(&mut temp_config)?;
        if count == 0 {
            return Ok(0);
        }
        Self::persist_imported_servers(state, temp_config.mcp.servers.as_ref(), &AppType::Codex)
    }

    /// 从 Gemini 导入 MCP，不反向写回原生配置。
    pub fn import_from_gemini(state: &AppState) -> Result<usize, AppError> {
        let mut temp_config = crate::app_config::MultiAppConfig::default();
        let count = crate::mcp::import_from_gemini(&mut temp_config)?;
        if count == 0 {
            return Ok(0);
        }
        Self::persist_imported_servers(state, temp_config.mcp.servers.as_ref(), &AppType::Gemini)
    }

    /// 从 Grok Build 导入 MCP，不反向写回原生配置。
    pub fn import_from_grokbuild(state: &AppState) -> Result<usize, AppError> {
        let mut temp_config = crate::app_config::MultiAppConfig::default();
        let count = crate::mcp::import_from_grokbuild(&mut temp_config)?;
        if count == 0 {
            return Ok(0);
        }
        Self::persist_imported_servers(state, temp_config.mcp.servers.as_ref(), &AppType::GrokBuild)
    }

    /// 从 OpenCode 导入 MCP，不反向写回原生配置。
    pub fn import_from_opencode(state: &AppState) -> Result<usize, AppError> {
        let mut temp_config = crate::app_config::MultiAppConfig::default();
        let count = crate::mcp::import_from_opencode(&mut temp_config)?;
        if count == 0 {
            return Ok(0);
        }
        Self::persist_imported_servers(state, temp_config.mcp.servers.as_ref(), &AppType::OpenCode)
    }

    /// 从 Hermes 导入 MCP，不反向写回原生配置。
    pub fn import_from_hermes(state: &AppState) -> Result<usize, AppError> {
        let mut temp_config = crate::app_config::MultiAppConfig::default();
        let count = crate::mcp::import_from_hermes(&mut temp_config)?;
        if count == 0 {
            return Ok(0);
        }
        Self::persist_imported_servers(state, temp_config.mcp.servers.as_ref(), &AppType::Hermes)
    }

    /// Merge by ID without replacing user metadata or projecting live config.
    /// Native readers, per-app error handling and database write boundaries stay
    /// with their existing owners; this only removes the duplicated merge loop.
    fn persist_imported_servers(
        state: &AppState,
        servers: Option<&HashMap<String, McpServer>>,
        app: &AppType,
    ) -> Result<usize, AppError> {
        let Some(servers) = servers else {
            return Ok(0);
        };
        let mut existing = state.db.get_all_mcp_servers()?;
        let mut new_count = 0;
        for server in servers.values() {
            let to_save = if let Some(existing_server) = existing.get(&server.id) {
                let mut merged = existing_server.clone();
                merged.apps.set_enabled_for(app, true);
                merged
            } else {
                new_count += 1;
                server.clone()
            };
            state.db.save_mcp_server(&to_save)?;
            existing.insert(to_save.id.clone(), to_save);
        }
        Ok(new_count)
    }

    /// 从所有支持 MCP 的应用导入服务器，返回新导入的数量。
    ///
    /// Best-effort：单个应用导入失败（如坏 config.toml）不阻断其余应用；
    /// 全部跑完后若有失败，聚合成一个错误上报——历史实现逐应用
    /// `unwrap_or(0)` 吞错，坏文件只会表现为"导入成功 0 个"，用户
    /// 无从得知哪个应用出了问题。
    pub fn import_from_all_apps(state: &AppState) -> Result<usize, AppError> {
        let mut total = 0;
        let mut failures: Vec<String> = Vec::new();

        let results: [(&str, Result<usize, AppError>); 6] = [
            ("claude", Self::import_from_claude(state)),
            ("codex", Self::import_from_codex(state)),
            ("gemini", Self::import_from_gemini(state)),
            ("grokbuild", Self::import_from_grokbuild(state)),
            ("opencode", Self::import_from_opencode(state)),
            ("hermes", Self::import_from_hermes(state)),
        ];
        for (app, result) in results {
            match result {
                Ok(count) => total += count,
                Err(err) => {
                    log::warn!("从 {app} 导入 MCP 失败: {err}");
                    failures.push(format!("{app}: {err}"));
                }
            }
        }

        if failures.is_empty() {
            Ok(total)
        } else {
            Err(AppError::Message(format!(
                "已导入 {total} 个，部分应用导入失败: {}",
                failures.join("; ")
            )))
        }
    }
}

#[cfg(test)]
mod toggle_lock_tests {
    use super::*;
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    #[test]
    fn mcp_toggle_waits_for_same_app_switch_but_not_other_apps() {
        // No server is stored: exercise lock admission without touching live files.
        let state = AppState::new(Arc::new(crate::database::Database::memory().unwrap())).unwrap();
        let switch_guard = futures::executor::block_on(
            state
                .proxy_service
                .lock_switch_for_app(AppType::Claude.as_str()),
        );
        let mut toggle = Box::pin(McpService::toggle_app_locked(
            &state,
            "missing",
            AppType::Claude,
            true,
        ));
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(matches!(toggle.as_mut().poll(&mut cx), Poll::Pending));

        let mut other_app = Box::pin(McpService::toggle_app_locked(
            &state,
            "missing",
            AppType::Codex,
            true,
        ));
        assert!(matches!(
            other_app.as_mut().poll(&mut cx),
            Poll::Ready(Ok(()))
        ));

        // A caller already holding the switch lock must use the synchronous core.
        McpService::toggle_app(&state, "missing", AppType::Claude, false).unwrap();
        drop(switch_guard);
        assert!(matches!(toggle.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
        assert!(matches!(
            Box::pin(
                state
                    .proxy_service
                    .lock_switch_for_app(AppType::Claude.as_str())
            )
            .as_mut()
            .poll(&mut cx),
            Poll::Ready(_)
        ));
    }
}
