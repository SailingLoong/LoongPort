//! Deep link import functionality
//!
//! scheme 是 `loongport://`（`tauri.conf.json` 的 `plugins.deep-link`）。
//!
//! **LoongPort 的主流程不走这里**：拿到 sk 与 endpoint 之后直接写 provider 记录
//! （见 `commands::relay::relay_provision`），不经 deeplink 导入。这条链路是从上游
//! 继承下来的通用导入能力，留着不碍事。
//!
//! Supports importing:
//! - Provider configurations (Claude/Codex/Gemini)
//! - MCP server configurations
//! - Prompts
//! - Skills
//!

/// 本 app 注册的 scheme（`tauri.conf.json` 的 `plugins.deep-link`）。
pub const APP_SCHEME: &str = "loongport";

mod mcp;
mod parser;
mod prompt;
mod provider;
mod skill;
mod utils;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

// Re-export public API
pub use mcp::import_mcp_from_deeplink;
pub use parser::parse_deeplink_url;
pub use prompt::import_prompt_from_deeplink;
pub use provider::{import_provider_from_deeplink, parse_and_merge_config};
// LoongPort 加的一行：`relay::provision` 复用这套「按 CLI 分派 settings_config」的构造，
// 免得自己再写一份 8 分支的 match（上游加新 CLI 时我们免费拿到）。
pub(crate) use provider::build_provider_from_request;
pub use skill::import_skill_from_deeplink;

/// Deep link import request model
///
/// Represents a parsed ccswitch:// URL ready for processing.
/// This struct contains all possible fields for all resource types.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeepLinkImportRequest {
    /// Protocol version (e.g., "v1")
    pub version: String,
    /// Resource type to import: "provider" | "prompt" | "mcp" | "skill"
    pub resource: String,

    // ============ Common fields ============
    /// Target application (claude/codex/gemini) - for provider, prompt, skill
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Resource name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether to enable after import (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    // ============ Provider-specific fields ============
    /// Provider homepage URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// API endpoint/base URL (supports comma-separated multiple URLs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// API key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Optional provider icon name (maps to built-in SVG)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Optional model name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional notes/description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Optional Haiku model (Claude only, v3.7.1+)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub haiku_model: Option<String>,
    /// Optional Sonnet model (Claude only, v3.7.1+)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sonnet_model: Option<String>,
    /// Optional Opus model (Claude only, v3.7.1+)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opus_model: Option<String>,
    /// Optional Fable model (Claude only) — writes `ANTHROPIC_DEFAULT_FABLE_MODEL`.
    ///
    /// LoongPort 新增（上游只有 haiku/sonnet/opus 三个别名）。加它是因为官网直连的
    /// DeepSeek 有 pro / flash 两档真实模型，要按角色分档写入
    /// （`vendor::deepseek::config_for`），而 deeplink 是生成配置的唯一通路。
    ///
    /// **上游本来就认这个 env** —— `proxy/model_mapper.rs` 读 `fable_model`
    /// 与 `subagent_model`，只是 deeplink 这条路没给传。所以这不是发明新键，
    /// 是把已有的键接到 deeplink 上。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fable_model: Option<String>,
    /// Optional subagent model (Claude only) — writes `CLAUDE_CODE_SUBAGENT_MODEL`.
    ///
    /// ⚠️ **这个键不在 `ANTHROPIC_DEFAULT_*` 系列里**，照抄前缀会写出一个
    /// Claude Code 不认的名字。见 `fable_model` 那段说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<String>,
    /// Claude 系鉴权走 `ANTHROPIC_API_KEY`（x-api-key）而不是默认的
    /// `ANTHROPIC_AUTH_TOKEN`（Bearer），且**完全不写** AUTH_TOKEN ——
    /// 两个字段同写时 Claude Code 优先 Bearer，被只认 x-api-key 的网关
    /// 静默忽略后就是一条必 401 的配置（opencode Go 网关实测如此）。
    ///
    /// LoongPort 新增；生成侧的传入口是 `relay::provision::ProvisionStyle`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_api_key_auth: Option<bool>,

    // ============ Prompt-specific fields ============
    /// Base64 encoded Markdown content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Prompt description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    // ============ MCP-specific fields ============
    /// Target applications for MCP (comma-separated: "claude,codex,gemini")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<String>,

    // ============ Skill-specific fields ============
    /// GitHub repository (format: "owner/name")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Skill directory name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,

    // ============ Config file fields (v3.8+) ============
    /// Base64 encoded config content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Config format (json/toml)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_format: Option<String>,
    /// Remote config URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_url: Option<String>,

    // ============ Usage script fields (v3.9+) ============
    /// Whether to enable usage query. Defaults to **disabled** — carrying a script
    /// is not itself a decision to run it; the link must say `usageEnabled=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_enabled: Option<bool>,
    /// Base64 encoded usage query script code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_script: Option<String>,
    /// Usage query API key (if different from provider API key)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_api_key: Option<String>,
    /// Usage query base URL (if different from provider endpoint)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_base_url: Option<String>,
    /// Usage query access token (for NewAPI template)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_access_token: Option<String>,
    /// Usage query user ID (for NewAPI template)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_user_id: Option<String>,
    /// Auto query interval in minutes (0 to disable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_auto_interval: Option<u64>,
}

#[cfg(test)]
mod scheme_consistency_tests {
    use super::APP_SCHEME;

    /// deeplink 的 scheme 声明在**三处**，任一处不同就静默失效（不报错、不崩溃，
    /// 只是导入链接点了没反应）。2026-08-02 踩过：`Info.plist` 漏改还写着上游的
    /// `ccswitch`，而这里与 `tauri.conf.json` 都已是 `loongport` ⇒ 系统把
    /// `ccswitch://` 交给我们、代码不认；`loongport://` 系统压根不路由给我们。
    ///
    /// 这条测试读那两个文件做字面比对 —— 它们不是 Rust 代码，编译器管不到。
    #[test]
    fn scheme_matches_info_plist_and_tauri_conf() {
        let plist = include_str!("../../Info.plist");
        assert!(
            plist.contains(&format!("<string>{APP_SCHEME}</string>")),
            "Info.plist 里注册的 scheme 与 APP_SCHEME ({APP_SCHEME}) 不一致 —— \
             deeplink 会静默失效"
        );

        let conf = include_str!("../../tauri.conf.json");
        let parsed: serde_json::Value =
            serde_json::from_str(conf).expect("tauri.conf.json 必须是合法 JSON");
        let schemes = parsed["plugins"]["deep-link"]["desktop"]["schemes"]
            .as_array()
            .expect("tauri.conf.json 必须声明 plugins.deep-link.desktop.schemes");
        assert!(
            schemes.iter().any(|s| s.as_str() == Some(APP_SCHEME)),
            "tauri.conf.json 的 schemes {schemes:?} 不含 APP_SCHEME ({APP_SCHEME})"
        );
    }
}
