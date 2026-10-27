//! MCP (Model Context Protocol) 服务器管理模块
//!
//! 本模块负责 MCP 服务器配置的验证、同步和导入导出。
//!
//! ## 模块结构
//!
//! - `validation` - 服务器配置验证
//! - `claude` - Claude MCP 同步和导入
//! - `codex` - Codex MCP 同步和导入（含 TOML 转换）
//! - `gemini` - Gemini MCP 同步和导入
//! - `opencode` - OpenCode MCP 同步和导入（含 local/remote 格式转换）
//! - `hermes` - Hermes MCP 同步和导入

use std::collections::HashMap;

use serde_json::Value;

use crate::app_config::{AppType, McpApps, McpServer};

mod claude;
mod codex;
mod gemini;
mod grokbuild;
mod hermes;
mod opencode;
mod validation;

// 重新导出公共 API
pub use claude::{
    import_from_claude, remove_server_from_claude, sync_enabled_to_claude,
    sync_single_server_to_claude,
};
pub use codex::{
    import_from_codex, remove_server_from_codex, sync_enabled_to_codex, sync_single_server_to_codex,
};
pub use gemini::{
    import_from_gemini, remove_server_from_gemini, sync_enabled_to_gemini,
    sync_single_server_to_gemini,
};
pub use grokbuild::{
    import_from_grokbuild, remove_server_from_grokbuild, sync_single_server_to_grokbuild,
};
pub use hermes::{import_from_hermes, remove_server_from_hermes, sync_single_server_to_hermes};
pub use opencode::{
    import_from_opencode, remove_server_from_opencode, sync_single_server_to_opencode,
};

/// Merge a validated native spec without replacing existing user-owned fields.
/// Native imports count new servers and newly enabled apps; service imports count only new IDs.
fn merge_imported_server(
    servers: &mut HashMap<String, McpServer>,
    id: &str,
    spec: &Value,
    app: &AppType,
) -> bool {
    let server = servers.entry(id.to_string()).or_insert_with(|| McpServer {
        id: id.to_string(),
        name: id.to_string(),
        server: spec.clone(),
        apps: McpApps::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: Vec::new(),
    });
    let changed = !server.apps.is_enabled_for(app);
    server.apps.set_enabled_for(app, true);
    changed
}
