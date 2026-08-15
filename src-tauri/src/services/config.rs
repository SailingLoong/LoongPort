use super::provider::{sanitize_claude_settings_for_live, ProviderService};
use crate::app_config::{AppType, MultiAppConfig};
use crate::error::AppError;
use crate::provider::Provider;
use chrono::Utc;
use serde_json::Value;
use std::fs;
use std::path::Path;

const MAX_BACKUPS: usize = 10;

/// 配置导入导出相关业务逻辑
pub struct ConfigService;

impl ConfigService {
    /// 为当前 config.json 创建备份，返回备份 ID（若文件不存在则返回空字符串）。
    pub fn create_backup(config_path: &Path) -> Result<String, AppError> {
        if !config_path.exists() {
            return Ok(String::new());
        }

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let backup_id = format!("backup_{timestamp}");

        let backup_dir = config_path
            .parent()
            .ok_or_else(|| AppError::Config("Invalid config path".into()))?
            .join("backups");

        fs::create_dir_all(&backup_dir).map_err(|e| AppError::io(&backup_dir, e))?;

        let backup_path = backup_dir.join(format!("{backup_id}.json"));
        let contents = fs::read(config_path).map_err(|e| AppError::io(config_path, e))?;
        fs::write(&backup_path, contents).map_err(|e| AppError::io(&backup_path, e))?;

        Self::cleanup_old_backups(&backup_dir, MAX_BACKUPS)?;

        Ok(backup_id)
    }

    fn cleanup_old_backups(backup_dir: &Path, retain: usize) -> Result<(), AppError> {
        if retain == 0 {
            return Ok(());
        }

        let entries = match fs::read_dir(backup_dir) {
            Ok(iter) => iter
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .map(|ext| ext == "json")
                        .unwrap_or(false)
                })
                .collect::<Vec<_>>(),
            Err(_) => return Ok(()),
        };

        if entries.len() <= retain {
            return Ok(());
        }

        let remove_count = entries.len().saturating_sub(retain);
        let mut sorted = entries;

        sorted.sort_by(|a, b| {
            let a_time = a.metadata().and_then(|m| m.modified()).ok();
            let b_time = b.metadata().and_then(|m| m.modified()).ok();
            a_time.cmp(&b_time)
        });

        for entry in sorted.into_iter().take(remove_count) {
            if let Err(err) = fs::remove_file(entry.path()) {
                log::warn!(
                    "Failed to remove old backup {}: {}",
                    entry.path().display(),
                    err
                );
            }
        }

        Ok(())
    }

    /// 同步当前供应商到对应的 live 配置。
    pub fn sync_current_providers_to_live(config: &mut MultiAppConfig) -> Result<(), AppError> {
        Self::sync_current_provider_for_app(config, &AppType::Claude)?;
        Self::sync_current_provider_for_app(config, &AppType::Codex)?;
        Self::sync_current_provider_for_app(config, &AppType::Gemini)?;
        Self::sync_current_provider_for_app(config, &AppType::GrokBuild)?;
        Ok(())
    }

    fn sync_current_provider_for_app(
        config: &mut MultiAppConfig,
        app_type: &AppType,
    ) -> Result<(), AppError> {
        let (current_id, provider) = {
            let manager = match config.get_manager(app_type) {
                Some(manager) => manager,
                None => return Ok(()),
            };

            if manager.current.is_empty() {
                return Ok(());
            }

            let current_id = manager.current.clone();
            let provider = match manager.providers.get(&current_id) {
                Some(provider) => provider.clone(),
                None => {
                    log::warn!(
                        "当前应用 {app_type:?} 的供应商 {current_id} 不存在，跳过 live 同步"
                    );
                    return Ok(());
                }
            };
            (current_id, provider)
        };

        match app_type {
            AppType::Codex => Self::sync_codex_live(config, &current_id, &provider)?,
            AppType::Claude => Self::sync_claude_live(config, &current_id, &provider)?,
            AppType::ClaudeDesktop => {
                // Claude Desktop 3P profiles are managed by claude_desktop_config.
            }
            AppType::CodexImage => {
                // 生图栏不写任何 live 配置：生图靠 `--mcp-image-gen` 那个 MCP 工具，
                // 它自己去库里读这一栏的 is_current 与其 sk。见 AppType::CodexImage 的文档。
            }
            AppType::Gemini => Self::sync_gemini_live(config, &current_id, &provider)?,
            AppType::GrokBuild => crate::grok_config::write_grok_provider_live(&provider)?,
            AppType::OpenCode => {
                // OpenCode uses additive mode, no live sync needed
                // OpenCode providers are managed directly in the config file
            }
            AppType::OpenClaw => {
                // OpenClaw uses additive mode, no live sync needed
                // OpenClaw providers are managed directly in the config file
            }
            AppType::Hermes => {
                // Hermes uses additive mode, no live sync needed
            }
            AppType::Pi => {
                // Pi owns its shared models/settings documents; this legacy
                // single-provider live-sync path must not rewrite them.
            }
        }

        Ok(())
    }

    fn sync_codex_live(
        config: &mut MultiAppConfig,
        provider_id: &str,
        provider: &Provider,
    ) -> Result<(), AppError> {
        let settings = provider.settings_config.as_object().ok_or_else(|| {
            AppError::Config(format!("供应商 {provider_id} 的 Codex 配置必须是对象"))
        })?;
        let auth = settings.get("auth").ok_or_else(|| {
            AppError::Config(format!("供应商 {provider_id} 的 Codex 配置缺少 auth 字段"))
        })?;
        if !auth.is_object() {
            return Err(AppError::Config(format!(
                "供应商 {provider_id} 的 Codex auth 配置必须是 JSON 对象"
            )));
        }
        let cfg_text = settings.get("config").and_then(Value::as_str);

        let profile = crate::proxy::providers::resolve_codex_catalog_tool_profile(provider);

        crate::codex_config::write_codex_provider_live_with_catalog(
            &provider.settings_config,
            provider.category.as_deref(),
            auth,
            cfg_text,
            profile,
        )?;
        // 注意：MCP 同步在 v3.7.0 中已通过 McpService 进行，不再在此调用
        // sync_enabled_to_codex 使用旧的 config.mcp.codex 结构，在新架构中为空
        // MCP 的启用/禁用应通过 McpService::toggle_app 进行

        let cfg_text_after = crate::codex_config::read_and_validate_codex_config_text()?;
        if let Some(manager) = config.get_manager_mut(&AppType::Codex) {
            if let Some(target) = manager.providers.get_mut(provider_id) {
                if let Some(obj) = target.settings_config.as_object_mut() {
                    let mut restored = serde_json::json!({
                        "auth": auth.clone(),
                        "config": cfg_text_after,
                    });
                    let restore_provider_token =
                        crate::codex_config::should_restore_codex_provider_token_for_backfill(
                            provider.category.as_deref(),
                            &provider.settings_config,
                        );
                    crate::codex_config::restore_codex_settings_for_backfill(
                        &mut restored,
                        &provider.settings_config,
                        restore_provider_token,
                    )?;
                    // 必须同时写回 auth 和 config: backfill 会把 live 的
                    // experimental_bearer_token 移到 restored.auth.OPENAI_API_KEY。
                    if let Some(restored_obj) = restored.as_object() {
                        if let Some(auth_value) = restored_obj.get("auth") {
                            obj.insert("auth".to_string(), auth_value.clone());
                        }
                        if let Some(config_value) = restored_obj.get("config") {
                            obj.insert("config".to_string(), config_value.clone());
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// 把 provider 的投影 `incoming` 合进磁盘现状 `existing`，只动 LoongPort own 的键。
    ///
    /// ## 为什么 Claude 这条要单独写一份（而不是照抄 Codex 的白名单）
    ///
    /// Codex 的 live 是 TOML、own 的是一组**顶层键**；Claude 的 live 是 JSON，
    /// own 的东西分两层：
    ///
    /// - **顶层**：provider 的 `settings_config` 里有什么就 own 什么（实测是
    ///   `env` / `model` / `skipDangerousModePermissionPrompt`，测试里还有 `permissions`）。
    ///   这份名单**跟着 provider 走**而不是写死 —— 用户新建的 provider 带什么字段，
    ///   那些字段就归它管，写死的名单会漏。
    /// - **`env` 内部**：`ANTHROPIC_*` 与 `CLAUDE_CODE_*` 是我们写的，其余是用户自己加的。
    ///   所以 `env` 必须**键级合并**，不能整个替换 —— 后者会抹掉用户的自定义变量。
    ///
    /// 我们写的那些 env 前缀里，`incoming` 没有的要**删掉**：否则切到不带
    /// `ANTHROPIC_DEFAULT_OPUS_MODEL` 的供应商时，上一个的值会留下来串台。
    fn merge_claude_owned_keys(existing: &Value, incoming: &Value) -> Value {
        let (Some(existing_map), Some(incoming_map)) = (existing.as_object(), incoming.as_object())
        else {
            // 磁盘上没有可用的 JSON 对象（首次写入 / 用户写坏了）：整体以投影为准。
            return incoming.clone();
        };

        let mut merged = existing_map.clone();
        for (key, value) in incoming_map {
            if key == "env" {
                continue;
            }
            merged.insert(key.clone(), value.clone());
        }

        let incoming_env = incoming_map.get("env").and_then(Value::as_object);
        let existing_env = existing_map.get("env").and_then(Value::as_object);
        if incoming_env.is_some() || existing_env.is_some() {
            let mut env = existing_env.cloned().unwrap_or_default();
            // 先清掉我们拥有的前缀，再按 incoming 重新写入 —— 这样「上一个供应商有、
            // 这一个没有」的变量不会残留，而用户自己加的变量原样留着。
            env.retain(|key, _| !Self::is_claude_owned_env_key(key));
            if let Some(incoming_env) = incoming_env {
                for (key, value) in incoming_env {
                    env.insert(key.clone(), value.clone());
                }
            }
            merged.insert("env".to_string(), Value::Object(env));
        }

        Value::Object(merged)
    }

    /// 这个 env 变量是 LoongPort 写的吗。
    ///
    /// 判前缀而不是列举全名：模型档位那组（`ANTHROPIC_DEFAULT_*_MODEL`）会随上游
    /// 新增档位而变长，列全名必然漏。
    fn is_claude_owned_env_key(key: &str) -> bool {
        key.starts_with("ANTHROPIC_") || key.starts_with("CLAUDE_CODE_")
    }

    fn sync_claude_live(
        config: &mut MultiAppConfig,
        provider_id: &str,
        provider: &Provider,
    ) -> Result<(), AppError> {
        use crate::config::{read_json_file, write_json_file};

        let settings_path = crate::config::get_claude_settings_path();
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }

        // 切换供应商是「投影」：只覆盖 provider 拥有的键，用户手写的其余内容原样保留。
        //
        // 此前这里全量覆盖 settings.json，于是用户自己加的顶层键（实测本机有 `language`）
        // 以及 `env` 里非 `ANTHROPIC_*` 的自定义变量，每切一次供应商就被静默抹掉。
        //
        // ⚠️ 合并只加在这条切换路径上。代理接管与备份恢复走 `ProxyService::write_claude_live`，
        // 那条必须保持全量覆盖 —— 恢复要精确还原，做合并会让接管写进去的键永远删不掉。
        let settings = sanitize_claude_settings_for_live(&provider.settings_config);
        let existing = read_json_file::<serde_json::Value>(&settings_path).unwrap_or(Value::Null);
        write_json_file(
            &settings_path,
            &Self::merge_claude_owned_keys(&existing, &settings),
        )?;

        let live_after = read_json_file::<serde_json::Value>(&settings_path)?;
        if let Some(manager) = config.get_manager_mut(&AppType::Claude) {
            if let Some(target) = manager.providers.get_mut(provider_id) {
                target.settings_config = live_after;
            }
        }

        Ok(())
    }

    fn sync_gemini_live(
        config: &mut MultiAppConfig,
        provider_id: &str,
        provider: &Provider,
    ) -> Result<(), AppError> {
        use crate::gemini_config::{env_to_json, read_gemini_env};

        ProviderService::write_gemini_live(provider)?;

        // 读回实际写入的内容并更新到配置中（包含 settings.json）
        let live_after_env = read_gemini_env()?;
        let settings_path = crate::gemini_config::get_gemini_settings_path();
        let live_after_config = if settings_path.exists() {
            crate::config::read_json_file(&settings_path)?
        } else {
            serde_json::json!({})
        };
        let mut live_after = env_to_json(&live_after_env);
        if let Some(obj) = live_after.as_object_mut() {
            obj.insert("config".to_string(), live_after_config);
        }

        if let Some(manager) = config.get_manager_mut(&AppType::Gemini) {
            if let Some(target) = manager.providers.get_mut(provider_id) {
                target.settings_config = live_after;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 用户手写的顶层键与自定义 env 变量必须活过供应商切换（本次修复的核心）。
    #[test]
    fn merge_keeps_user_authored_keys_and_custom_env() {
        let existing = json!({
            "language": "zh-CN",
            "hooks": { "Stop": "notify" },
            "env": {
                "ANTHROPIC_BASE_URL": "https://old.example",
                "MY_OWN_VAR": "keep-me"
            }
        });
        let incoming = json!({
            "env": { "ANTHROPIC_BASE_URL": "https://new.example" }
        });

        let merged = ConfigService::merge_claude_owned_keys(&existing, &incoming);

        assert_eq!(
            merged.get("language").and_then(|v| v.as_str()),
            Some("zh-CN")
        );
        assert!(
            merged.get("hooks").is_some(),
            "用户的 hooks 必须留着: {merged}"
        );
        assert_eq!(
            merged.pointer("/env/MY_OWN_VAR").and_then(|v| v.as_str()),
            Some("keep-me"),
            "用户自定义 env 变量必须留着: {merged}"
        );
        assert_eq!(
            merged
                .pointer("/env/ANTHROPIC_BASE_URL")
                .and_then(|v| v.as_str()),
            Some("https://new.example"),
            "我们拥有的 env 要按投影覆盖"
        );
    }

    /// 上一个供应商有、这一个没有的 ANTHROPIC_* 必须清掉，否则模型名串台。
    #[test]
    fn merge_drops_owned_env_keys_absent_from_incoming() {
        let existing = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://old.example",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "stale-opus",
                "CLAUDE_CODE_SUBAGENT_MODEL": "stale-subagent",
                "MY_OWN_VAR": "keep-me"
            }
        });
        let incoming = json!({
            "env": { "ANTHROPIC_BASE_URL": "https://new.example" }
        });

        let merged = ConfigService::merge_claude_owned_keys(&existing, &incoming);

        assert!(
            merged
                .pointer("/env/ANTHROPIC_DEFAULT_OPUS_MODEL")
                .is_none(),
            "got: {merged}"
        );
        assert!(merged.pointer("/env/CLAUDE_CODE_SUBAGENT_MODEL").is_none());
        assert_eq!(
            merged.pointer("/env/MY_OWN_VAR").and_then(|v| v.as_str()),
            Some("keep-me")
        );
    }

    /// 磁盘上没有可用 JSON 对象时整体以投影为准，不能卡死切换。
    #[test]
    fn merge_falls_back_to_incoming_without_usable_existing() {
        let incoming = json!({ "env": { "ANTHROPIC_BASE_URL": "https://new.example" } });

        for existing in [Value::Null, json!("not an object"), json!([1, 2])] {
            assert_eq!(
                ConfigService::merge_claude_owned_keys(&existing, &incoming),
                incoming
            );
        }
    }
}
