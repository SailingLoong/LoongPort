use super::provider::{sanitize_claude_settings_for_live, ProviderService};
use crate::app_config::{AppType, MultiAppConfig};
use crate::error::AppError;
use crate::provider::Provider;
use crate::secrets::{files::OwnedFile, session::SecretSession};
use chrono::Utc;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::str::FromStr;

mod gemini;
pub use gemini::GeminiSharedDraft;

const MAX_BACKUPS: usize = 10;

/// 配置导入导出相关业务逻辑
pub struct ConfigService;

impl ConfigService {
    #[cfg(feature = "test-hooks")]
    pub fn verify_startup_upgrade_auth() -> Result<(), AppError> {
        crate::secrets::upgrade::auth_tests::verify()
    }

    #[cfg(feature = "test-hooks")]
    pub fn verify_startup_upgrade_review() -> Result<(), AppError> {
        crate::secrets::upgrade::review_tests::verify()
    }

    /// 为当前 config.json 创建备份，返回备份 ID（若文件不存在则返回空字符串）。
    pub fn create_backup(session: &SecretSession, config_path: &Path) -> Result<String, AppError> {
        let source = OwnedFile::at_path(session, config_path)?;
        if source.relative_path() != Path::new("config.json") {
            return Err(AppError::Config("secret.unregistered_file".into()));
        }
        match fs::symlink_metadata(config_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(error) => return Err(AppError::io(config_path, error)),
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                return Err(AppError::Config("secret.invalid_storage_path".into()))
            }
            Ok(_) => {}
        }
        let contents = source.read(session)?;

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let backup_id = format!("backup_{timestamp}");

        let backup_dir = config_path
            .parent()
            .ok_or_else(|| AppError::Config("Invalid config path".into()))?
            .join("backups");

        fs::create_dir_all(&backup_dir).map_err(|e| AppError::io(&backup_dir, e))?;

        let backup_path = backup_dir.join(format!("{backup_id}.json"));
        OwnedFile::at_path(session, &backup_path)?.write(session, &contents)?;

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
                    entry.file_name().to_str().is_some_and(|name| {
                        name.starts_with("backup_")
                            && OwnedFile::registered(std::path::Path::new("backups").join(name))
                                .is_ok()
                    })
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
            Some(provider),
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

// Original snippet command mutations. Validation stays at the command boundary;
// both compatibility entry points share the same version admission below.
impl ConfigService {
    fn admit_legacy_snippet_write(
        state: &crate::store::AppState,
        app_type: &str,
    ) -> Result<(), String> {
        if matches!(app_type, "claude" | "codex" | "gemini")
            && crate::mode::operation::uses_upstream4_schema(&state.db)
                .map_err(|e| e.to_string())?
        {
            return Err("mode.legacy_common_config_frozen".into());
        }
        Ok(())
    }

    pub(crate) fn set_common_config_snippet(
        state: &crate::store::AppState,
        app_type: &str,
        snippet: String,
    ) -> Result<(), String> {
        Self::admit_legacy_snippet_write(state, app_type)?;
        let is_cleared = snippet.trim().is_empty();
        let old_snippet = state
            .db
            .get_config_snippet(app_type)
            .map_err(|e| e.to_string())?;

        let value = if is_cleared { None } else { Some(snippet) };

        if matches!(app_type, "claude" | "codex" | "gemini") {
            if let Some(legacy_snippet) = old_snippet
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                let app = AppType::from_str(app_type).map_err(|e| e.to_string())?;
                crate::services::provider::ProviderService::migrate_legacy_common_config_usage(
                    state,
                    app,
                    legacy_snippet,
                )
                .map_err(|e| e.to_string())?;
            }
        }

        state
            .db
            .set_config_snippet(app_type, value)
            .map_err(|e| e.to_string())?;
        state
            .db
            .set_config_snippet_cleared(app_type, is_cleared)
            .map_err(|e| e.to_string())?;

        if matches!(app_type, "claude" | "codex" | "gemini") {
            let app = AppType::from_str(app_type).map_err(|e| e.to_string())?;
            crate::services::provider::ProviderService::sync_current_provider_for_app(state, app)
                .map_err(|e| e.to_string())?;
        }

        if app_type == "omo"
            && state
                .db
                .get_current_omo_provider("opencode", "omo")
                .map_err(|e| e.to_string())?
                .is_some()
        {
            crate::services::OmoService::write_config_to_file(
                state,
                &crate::services::omo::STANDARD,
            )
            .map_err(|e| e.to_string())?;
        }
        if app_type == "omo-slim"
            && state
                .db
                .get_current_omo_provider("opencode", "omo-slim")
                .map_err(|e| e.to_string())?
                .is_some()
        {
            crate::services::OmoService::write_config_to_file(state, &crate::services::omo::SLIM)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn set_claude_common_config_snippet(
        state: &crate::store::AppState,
        snippet: String,
    ) -> Result<(), String> {
        Self::admit_legacy_snippet_write(state, "claude")?;
        let is_cleared = snippet.trim().is_empty();

        let value = if is_cleared { None } else { Some(snippet) };

        state
            .db
            .set_config_snippet("claude", value)
            .map_err(|e| e.to_string())?;
        state
            .db
            .set_config_snippet_cleared("claude", is_cleared)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

// U05 edits client-global fields through the existing app journal. Provider
// records, mode, authentication and catalog files are not save targets here.
fn shared_problem(code: &str) -> AppError {
    AppError::Config(format!("shared_settings.{code}"))
}
fn shared_code(error: &AppError) -> &'static str {
    match error {
        AppError::Config(code) if code == "shared_settings.readOnlyFields" => "readOnlyFields",
        AppError::Config(code) if code == "shared_settings.invalidFormat" => "invalidFormat",
        AppError::Config(code) if code.contains("locked") => "vaultLocked",
        AppError::Config(code)
            if code == "live.verification_required" || code == "mode.verification_required" =>
        {
            "pendingOperation"
        }
        AppError::Conflict(_) => "sourceChanged",
        AppError::Config(code) if code == "shared_settings.legacyDisabled" => "legacyDisabled",
        _ => "ownerUnavailable",
    }
}
fn shared_key_allowed(path: &[String]) -> bool {
    use crate::live::floor;
    // Existing provider/auth/catalog and MCP owners remain the only writers of
    // these namespaces, including active-profile route overrides.
    !path.iter().any(|key| {
        ProviderService::is_sensitive_config_key(key)
            || matches!(
                key.to_ascii_lowercase().as_str(),
                "authorization" | "http_headers" | "env_http_headers" | "chatgpt_base_url"
            )
            || floor::CODEX_FLOOR_TOP.contains(&key.as_str())
            || floor::CODEX_EXCLUSIVE_TOP.contains(&key.as_str())
    }) && !matches!(
        path.first().map(String::as_str),
        Some(
            "model_providers"
                | "mcp_servers"
                | "mcp"
                | "auth"
                | "profile"
                | "profiles"
                | "forced_login_method"
                | "forced_chatgpt_workspace_id"
        )
    ) && !floor::CODEX_FLOOR_NESTED
        .iter()
        .any(|owned| path.iter().map(String::as_str).eq(owned.iter().copied()))
}
fn shared_value_allowed(value: &toml_edit::Value, path: &[String]) -> bool {
    match value {
        toml_edit::Value::InlineTable(table) => table.iter().all(|(key, value)| {
            let mut child = path.to_vec();
            child.push(key.into());
            shared_key_allowed(&child) && shared_value_allowed(value, &child)
        }),
        toml_edit::Value::Array(array) => {
            array.iter().all(|value| shared_value_allowed(value, path))
        }
        _ => true,
    }
}
fn shared_projection(table: &mut dyn toml_edit::TableLike, parent: &[String]) {
    let keys = table
        .iter()
        .map(|(key, _)| key.to_owned())
        .collect::<Vec<_>>();
    for key in keys {
        let mut path = parent.to_vec();
        path.push(key.clone());
        let item = table.get_mut(&key).unwrap();
        if !shared_key_allowed(&path) {
            table.remove(&key);
            continue;
        }
        if let Some(nested) = item.as_table_like_mut() {
            shared_projection(nested, &path);
        } else if item
            .as_value()
            .is_none_or(|value| !shared_value_allowed(value, &path))
        {
            table.remove(&key);
        }
    }
}
fn shared_fields(
    table: &dyn toml_edit::TableLike,
    parent: &[String],
    fields: &mut std::collections::BTreeMap<Vec<String>, toml_edit::Item>,
) -> Result<(), AppError> {
    for (key, item) in table.iter() {
        let mut path = parent.to_vec();
        path.push(key.into());
        if !shared_key_allowed(&path) {
            return Err(shared_problem("readOnlyFields"));
        }
        if let Some(nested) = item.as_table_like() {
            shared_fields(nested, &path, fields)?;
        } else if let Some(value) = item.as_value() {
            if !shared_value_allowed(value, &path) {
                return Err(shared_problem("readOnlyFields"));
            }
            fields.insert(path, item.clone());
        } else {
            return Err(shared_problem("invalidFormat"));
        }
    }
    Ok(())
}
fn shared_parse(text: &str) -> Result<toml_edit::DocumentMut, AppError> {
    text.parse().map_err(|_| shared_problem("invalidFormat"))
}
fn shared_item_value(item: &toml_edit::Item) -> Result<toml::Value, AppError> {
    toml::from_str::<toml::Value>(&format!("value = {item}\n"))
        .map(|value| value["value"].clone())
        .map_err(|_| shared_problem("invalidFormat"))
}
// Shared editors reuse the transaction and receipt, while
// each native format retains its own field projection and patch implementation.
#[derive(Clone, Copy)]
enum SharedApp {
    Codex,
    Claude,
    Gemini,
}
impl SharedApp {
    fn kind(self) -> AppType {
        match self {
            Self::Codex => AppType::Codex,
            Self::Claude => AppType::Claude,
            Self::Gemini => AppType::Gemini,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
        }
    }
    fn scope(self) -> crate::mode::state::SaveScope {
        match self {
            Self::Codex => crate::mode::state::SaveScope::CodexSharedSettings,
            Self::Claude => crate::mode::state::SaveScope::ClaudeSharedSettings,
            Self::Gemini => crate::mode::state::SaveScope::GeminiSharedSettings,
        }
    }
    fn sources(self) -> Result<Vec<SharedSource>, AppError> {
        use crate::live::engine::{read_current, LiveFile};
        let files = match self {
            Self::Codex => vec![(
                "codexConfig",
                LiveFile::private(crate::codex_config::get_codex_config_path()),
            )],
            Self::Claude => vec![(
                "claudeSettings",
                LiveFile::private(crate::config::get_claude_settings_path()),
            )],
            Self::Gemini => vec![
                (
                    "geminiEnv",
                    LiveFile::private(crate::gemini_config::get_gemini_env_path()),
                ),
                (
                    "geminiSettings",
                    LiveFile::shared(crate::gemini_config::get_gemini_settings_path()),
                ),
            ],
        };
        files
            .into_iter()
            .map(|(role, file)| {
                let pre = read_current(&file.path)?;
                Ok(SharedSource { role, file, pre })
            })
            .collect()
    }
    fn config_text(self, bytes: Option<&[u8]>) -> Result<String, AppError> {
        match self {
            Self::Codex => shared_config_text(shared_parse(
                std::str::from_utf8(bytes.unwrap_or_default())
                    .map_err(|_| shared_problem("invalidFormat"))?,
            )?),
            Self::Claude => {
                let mut value =
                    claude_shared_parse(&crate::config::get_claude_settings_path(), bytes)?;
                claude_shared_projection(&mut value, &[], claude_shared_field_allowed);
                serde_json::to_string_pretty(&value).map_err(|_| shared_problem("invalidFormat"))
            }
            Self::Gemini => Err(shared_problem("invalidFormat")),
        }
    }
}
struct SharedSource {
    role: &'static str,
    file: crate::live::engine::LiveFile,
    pre: Option<Vec<u8>>,
}
#[derive(serde::Serialize)]
#[serde(untagged)]
enum SharedDraft {
    Text(String),
    Gemini(GeminiSharedDraft),
}
impl SharedDraft {
    fn encoded(&self) -> Result<String, AppError> {
        match self {
            Self::Text(text) => Ok(text.clone()),
            Self::Gemini(draft) => {
                serde_json::to_string(draft).map_err(|_| shared_problem("invalidFormat"))
            }
        }
    }
}
struct SharedPlan {
    request: crate::mode::state::SaveRequest,
    files: Vec<(
        &'static str,
        crate::live::engine::LiveFile,
        crate::live::patch::Guarded,
    )>,
    changes: Vec<Value>,
}
impl SharedPlan {
    fn preview(&self, app: SharedApp) -> Value {
        let files: Vec<_> = self.files.iter().map(|(role,_,patch)| {
            let post = match &patch.then {
                crate::live::patch::WholeFile::Write(bytes) => Some(bytes.as_slice()),
                crate::live::patch::WholeFile::Delete => None,
            };
            serde_json::json!({"role":role,"change":if crate::live::engine::digest(post)==patch.expected_pre {"unchanged"}else{"write"}})
        }).collect();
        serde_json::json!({"app":app.name(),"request":self.request,"status":"ready","changes":self.changes,"files":files})
    }
}
fn shared_draft(app: SharedApp, sources: &[SharedSource]) -> Result<SharedDraft, AppError> {
    if matches!(app, SharedApp::Gemini) {
        Ok(SharedDraft::Gemini(gemini::read(sources)?))
    } else {
        Ok(SharedDraft::Text(
            app.config_text(sources[0].pre.as_deref())?,
        ))
    }
}

fn shared_result(
    app: SharedApp,
    request: &crate::mode::state::SaveRequest,
    status: &str,
    code: Option<&str>,
) -> Value {
    let mut value = serde_json::json!({"app":app.name(),"request":request,"status":status});
    if let Some(code) = code {
        value["code"] = serde_json::json!(code);
    }
    value
}
fn shared_request(
    app: SharedApp,
    config: &str,
    id: &str,
) -> Result<crate::mode::state::SaveRequest, AppError> {
    let digest = crate::live::engine::sha256_hex(config.as_bytes());
    let request = crate::mode::state::SaveRequest {
        id: id.into(),
        scope: app.scope(),
        provider_id: None,
        draft_digest: digest.clone(),
        revision: digest,
    };
    request.validate()?;
    Ok(request)
}
fn shared_original(
    app: SharedApp,
    state: &crate::store::AppState,
) -> Result<Option<(crate::mode::state::SaveRequest, &'static str)>, AppError> {
    use crate::mode::{operation, state as mode};
    let vault = state.db.secret_session().read()?;
    let live = mode::load_app(
        &crate::live::engine::DeviceStore::for_device(),
        &vault,
        app.name(),
    )?;
    live.apps
        .get(app.name())
        .map(|entry| {
            operation::saved_request_view(app.name(), entry, app.scope(), None)
                .map(|view| view.map(|v| (v.request, v.status)))
        })
        .transpose()
        .map(Option::flatten)
}
fn shared_query(
    app: SharedApp,
    state: &crate::store::AppState,
    request: &crate::mode::state::SaveRequest,
) -> Result<Value, AppError> {
    request.validate()?;
    if request.scope != app.scope() {
        return Err(shared_problem("readOnlyFields"));
    }
    let vault = state.db.secret_session().read()?;
    let live = crate::mode::state::load_app(
        &crate::live::engine::DeviceStore::for_device(),
        &vault,
        app.name(),
    )?;
    let status = live
        .apps
        .get(app.name())
        .map(|entry| crate::mode::operation::saved_request_status(app.name(), entry, request))
        .transpose()?
        .unwrap_or("notRecorded");
    Ok(shared_result(app, request, status, None))
}

fn shared_plan(
    app: SharedApp,
    state: &crate::store::AppState,
    config: &SharedDraft,
    id: &str,
) -> Result<SharedPlan, AppError> {
    let _admission =
        crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &app.kind())?;
    shared_plan_sources(app, config, id, app.sources()?)
}
fn shared_plan_sources(
    app: SharedApp,
    config: &SharedDraft,
    id: &str,
    mut sources: Vec<SharedSource>,
) -> Result<SharedPlan, AppError> {
    match (app, config) {
        (SharedApp::Gemini, SharedDraft::Gemini(draft)) => gemini::plan(draft, id, sources),
        (SharedApp::Codex | SharedApp::Claude, SharedDraft::Text(text)) => {
            let source = sources.remove(0);
            let (request, file, patch, changes) =
                plan_shared_source(app, text, id, source.file, source.pre)?;
            Ok(SharedPlan {
                request,
                files: vec![(source.role, file, patch)],
                changes,
            })
        }
        _ => Err(shared_problem("invalidFormat")),
    }
}

// Both editors plan from one observed file version; importing must not seed
// against one snapshot and silently publish a diff against a newer one.
fn shared_plan_from_source(
    config: &str,
    id: &str,
    file: crate::live::engine::LiveFile,
    pre: Option<Vec<u8>>,
) -> Result<
    (
        crate::mode::state::SaveRequest,
        crate::live::engine::LiveFile,
        crate::live::patch::Guarded,
        Vec<Value>,
    ),
    AppError,
> {
    use crate::live::{
        engine,
        patch::{toml::TomlPatch, Guarded, KeyPath, LivePatch, WholeFile},
    };
    let mut request = shared_request(SharedApp::Codex, config, id)?;
    let before_text = std::str::from_utf8(pre.as_deref().unwrap_or_default())
        .map_err(|_| shared_problem("invalidFormat"))?;
    let full = shared_parse(before_text)?;
    let mut before = full.clone();
    shared_projection(before.as_table_mut(), &[]);
    let after = shared_parse(config)?;
    let mut old = std::collections::BTreeMap::new();
    let mut new = std::collections::BTreeMap::new();
    shared_fields(before.as_table(), &[], &mut old)?;
    shared_fields(after.as_table(), &[], &mut new)?;
    let mut patch = TomlPatch::default();
    let mut changes = std::collections::BTreeMap::new();
    for (path, value) in &new {
        let before_value = old.get(path).map(shared_item_value).transpose()?;
        let after_value = shared_item_value(value)?;
        if before_value.as_ref() != Some(&after_value) {
            // A value must not replace a table/array whose protected children
            // were deliberately omitted from the shared editor's projection.
            if let Some(item) = path
                .iter()
                .try_fold(full.as_item(), |item, key| item.get(key.as_str()))
            {
                let editable = if let Some(table) = item.as_table_like() {
                    shared_fields(table, path, &mut std::collections::BTreeMap::new()).is_ok()
                } else {
                    item.as_value()
                        .is_some_and(|value| shared_value_allowed(value, path))
                };
                if !editable {
                    return Err(shared_problem("readOnlyFields"));
                }
            }
            patch.set.push((KeyPath(path.clone()), value.clone()));
            let mut change =
                serde_json::json!({"field":path.join("."),"after":after_value.to_string()});
            if let Some(before) = before_value {
                change["before"] = serde_json::json!(before.to_string());
            }
            changes.insert(path.clone(), change);
        }
    }
    for path in old.keys().filter(|path| !new.contains_key(*path)) {
        patch.remove.push(KeyPath(path.clone()));
        changes.insert(path.clone(), serde_json::json!({"field":path.join("."),"before":shared_item_value(&old[path])?.to_string()}));
    }
    let bytes = patch
        .apply(&file.path, pre.as_deref())
        .map_err(|_| shared_problem("invalidFormat"))?;
    let source = serde_json::to_vec(&(
        "codexSharedSettings",
        &file.path,
        engine::digest(pre.as_deref()),
    ))
    .map_err(|_| shared_problem("invalidFormat"))?;
    request.revision = engine::sha256_hex(&source);
    Ok((
        request,
        file,
        Guarded {
            expected_pre: engine::digest(pre.as_deref()),
            then: WholeFile::Write(bytes),
        },
        changes.into_values().collect(),
    ))
}
fn shared_import_snippet(
    app: SharedApp,
    state: &crate::store::AppState,
) -> Result<Option<String>, AppError> {
    if state.db.is_config_snippet_cleared(app.name())? {
        return Err(shared_problem("legacyDisabled"));
    }
    let Some(snippet) = state
        .db
        .get_config_snippet(app.name())?
        .filter(|s| !s.trim().is_empty())
    else {
        return Ok(None);
    };
    let current = ProviderService::current(state, app.kind())?;
    let provider = state.db.get_provider_by_id(&current, app.name())?;
    if !provider.as_ref().is_some_and(|provider| {
        crate::services::provider::provider_uses_common_config(
            &app.kind(),
            provider,
            Some(&snippet),
        )
    }) {
        return Err(shared_problem("legacyDisabled"));
    }
    Ok(Some(snippet))
}
fn shared_import_fields(
    state: &crate::store::AppState,
) -> Result<std::collections::BTreeMap<Vec<String>, toml_edit::Item>, AppError> {
    let Some(snippet) = shared_import_snippet(SharedApp::Codex, state)? else {
        return Ok(Default::default());
    };
    let mut doc = shared_parse(&snippet)?;
    shared_projection(doc.as_table_mut(), &[]);
    let mut fields = std::collections::BTreeMap::new();
    shared_fields(doc.as_table(), &[], &mut fields)?;
    Ok(fields)
}

fn shared_path_missing(doc: &toml_edit::DocumentMut, path: &[String]) -> bool {
    let mut item = doc.as_item();
    for key in path {
        let Some(table) = item.as_table_like() else {
            return false;
        };
        let Some(next) = table.get(key) else {
            return true;
        };
        item = next;
    }
    false
}

fn shared_config_text(mut doc: toml_edit::DocumentMut) -> Result<String, AppError> {
    shared_projection(doc.as_table_mut(), &[]);
    let values = toml::from_str::<toml::Table>(&doc.to_string())
        .map_err(|_| shared_problem("invalidFormat"))?;
    toml::to_string(&values).map_err(|_| shared_problem("invalidFormat"))
}

// Claude fields retain their native owner rules. JSON is never converted to TOML.
fn claude_shared_key_allowed(path: &[String]) -> bool {
    use crate::live::floor;
    if path.iter().any(|key| {
        ProviderService::is_sensitive_config_key(key)
            || matches!(
                key.to_ascii_lowercase().as_str(),
                "authorization"
                    | "headers"
                    | "http_headers"
                    | "httpheaders"
                    | "env_http_headers"
                    | "envhttpheaders"
            )
    }) {
        return false;
    }
    match path {
        [key] => {
            !floor::claude_floor_top(key)
                && !matches!(
                    key.as_str(),
                    "mcpServers"
                        | "mcp_servers"
                        | "mcp"
                        | "enabledMcpjsonServers"
                        | "disabledMcpjsonServers"
                        | "enableAllProjectMcpServers"
                        | "allowedMcpServers"
                        | "deniedMcpServers"
                )
        }
        [parent, key, ..] if parent == "env" => {
            !floor::claude_floor_env(key) && !floor::claude_exclusive_env(key)
        }
        _ => true,
    }
}
fn claude_shared_field_allowed(path: &[String], value: &Value) -> bool {
    claude_shared_key_allowed(path) && (path != ["env"] || value.is_object())
}
fn claude_shared_value_allowed(
    value: &Value,
    path: &[String],
    allowed: fn(&[String], &Value) -> bool,
) -> bool {
    if !allowed(path, value) {
        return false;
    }
    match value {
        Value::Object(map) => map.iter().all(|(key, value)| {
            let mut child = path.to_vec();
            child.push(key.clone());
            claude_shared_value_allowed(value, &child, allowed)
        }),
        Value::Array(array) => array
            .iter()
            .all(|value| claude_shared_value_allowed(value, path, allowed)),
        _ => true,
    }
}
fn claude_shared_projection(
    value: &mut Value,
    path: &[String],
    allowed: fn(&[String], &Value) -> bool,
) {
    if let Some(map) = value.as_object_mut() {
        for key in map.keys().cloned().collect::<Vec<_>>() {
            let mut child = path.to_vec();
            child.push(key.clone());
            let item = map.get_mut(&key).unwrap();
            if !allowed(&child, item) {
                map.shift_remove(&key);
                continue;
            }
            if item.is_object() {
                let was_empty = item.as_object().unwrap().is_empty();
                claude_shared_projection(item, &child, allowed);
                // An object emptied by redaction is not an editable empty object:
                // removing it from a draft must not remove its protected children.
                if !was_empty && item.as_object().unwrap().is_empty() {
                    map.shift_remove(&key);
                }
            } else if !claude_shared_value_allowed(item, &child, allowed) {
                map.shift_remove(&key);
            }
        }
    }
}
fn claude_shared_fields(
    value: &Value,
    path: &[String],
    fields: &mut std::collections::BTreeMap<Vec<String>, Value>,
    allowed: fn(&[String], &Value) -> bool,
) -> Result<(), AppError> {
    if !claude_shared_value_allowed(value, path, allowed) {
        return Err(shared_problem("readOnlyFields"));
    }
    if let Some(map) = value
        .as_object()
        .filter(|map| !map.is_empty() || path.is_empty())
    {
        for (key, value) in map {
            let mut child = path.to_vec();
            child.push(key.clone());
            claude_shared_fields(value, &child, fields, allowed)?;
        }
    } else {
        fields.insert(path.to_vec(), value.clone());
    }
    Ok(())
}
fn claude_shared_parse(path: &Path, bytes: Option<&[u8]>) -> Result<Value, AppError> {
    crate::live::patch::json::parse(path, bytes)
        .map(|(value, _)| value)
        .map_err(|_| shared_problem("invalidFormat"))
}
fn claude_import_fields(
    state: &crate::store::AppState,
) -> Result<std::collections::BTreeMap<Vec<String>, Value>, AppError> {
    let Some(text) = shared_import_snippet(SharedApp::Claude, state)? else {
        return Ok(Default::default());
    };
    let mut value = claude_shared_parse(
        &crate::config::get_claude_settings_path(),
        Some(text.as_bytes()),
    )?;
    claude_shared_projection(&mut value, &[], claude_shared_field_allowed);
    let mut fields = Default::default();
    claude_shared_fields(&value, &[], &mut fields, claude_shared_field_allowed)?;
    Ok(fields)
}
fn claude_path_missing(value: &Value, path: &[String]) -> bool {
    let mut current = value;
    for key in path {
        let Some(map) = current.as_object() else {
            return false;
        };
        let Some(next) = map.get(key) else {
            return true;
        };
        current = next;
    }
    false
}
fn plan_shared_source(
    app: SharedApp,
    config: &str,
    id: &str,
    file: crate::live::engine::LiveFile,
    pre: Option<Vec<u8>>,
) -> Result<
    (
        crate::mode::state::SaveRequest,
        crate::live::engine::LiveFile,
        crate::live::patch::Guarded,
        Vec<Value>,
    ),
    AppError,
> {
    if matches!(app, SharedApp::Codex) {
        return shared_plan_from_source(config, id, file, pre);
    }
    use crate::live::engine;
    let mut request = shared_request(app, config, id)?;
    let (patch, changes) = shared_json_patch(
        &file,
        pre.as_deref(),
        config,
        claude_shared_field_allowed,
        false,
    )?;
    let source = serde_json::to_vec(&(app.scope(), &file.path, engine::digest(pre.as_deref())))
        .map_err(|_| shared_problem("invalidFormat"))?;
    request.revision = engine::sha256_hex(&source);
    Ok((request, file, patch, changes))
}
fn shared_json_patch(
    file: &crate::live::engine::LiveFile,
    pre: Option<&[u8]>,
    config: &str,
    allowed: fn(&[String], &Value) -> bool,
    preserve_unchanged: bool,
) -> Result<(crate::live::patch::Guarded, Vec<Value>), AppError> {
    use crate::live::{
        engine,
        patch::{
            json::{value_at, JsonPatch},
            Guarded, KeyPath, LivePatch, WholeFile,
        },
    };
    let full = claude_shared_parse(&file.path, pre)?;
    let mut before = full.clone();
    claude_shared_projection(&mut before, &[], allowed);
    let after = claude_shared_parse(&file.path, Some(config.as_bytes()))?;
    let mut old = std::collections::BTreeMap::new();
    let mut new = std::collections::BTreeMap::new();
    claude_shared_fields(&before, &[], &mut old, allowed)?;
    claude_shared_fields(&after, &[], &mut new, allowed)?;
    let mut patch = JsonPatch::default();
    let mut changes = std::collections::BTreeMap::new();
    for (path, value) in &new {
        if old.get(path) != Some(value) {
            if value_at(&full, &KeyPath(path.clone()))
                .is_some_and(|value| !claude_shared_value_allowed(value, path, allowed))
            {
                return Err(shared_problem("readOnlyFields"));
            }
            patch.set.push((KeyPath(path.clone()), value.clone()));
            let mut change = serde_json::json!({"field":path.join("."),"after":value.to_string()});
            if let Some(value) = old.get(path) {
                change["before"] = serde_json::json!(value.to_string());
            }
            changes.insert(path.clone(), change);
        }
    }
    for (path, value) in old.iter().filter(|(path, _)| !new.contains_key(*path)) {
        patch.remove.push(KeyPath(path.clone()));
        changes.insert(
            path.clone(),
            serde_json::json!({"field":path.join("."),"before":value.to_string()}),
        );
    }
    let then = if preserve_unchanged && changes.is_empty() {
        pre.map_or(WholeFile::Delete, |bytes| WholeFile::Write(bytes.to_vec()))
    } else {
        WholeFile::Write(
            patch
                .apply(&file.path, pre)
                .map_err(|_| shared_problem("invalidFormat"))?,
        )
    };
    Ok((
        Guarded {
            expected_pre: engine::digest(pre),
            then,
        },
        changes.into_values().collect(),
    ))
}

impl ConfigService {
    fn read_shared_settings_import(
        app: SharedApp,
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.name()));
        let read = || -> Result<Vec<Value>, AppError> {
            let _admission =
                crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &app.kind())?;
            let sources = app.sources()?;
            let bytes = &sources[0].pre;
            match app {
                SharedApp::Codex => {
                    let doc = shared_parse(
                        std::str::from_utf8(bytes.as_deref().unwrap_or_default())
                            .map_err(|_| shared_problem("invalidFormat"))?,
                    )?;
                    shared_import_fields(state)?.into_iter().filter(|(path,_)|shared_path_missing(&doc,path))
                      .map(|(path,value)|Ok(serde_json::json!({"field":path.join("."),"path":path,"value":shared_item_value(&value)?.to_string()}))).collect()
                }
                SharedApp::Claude => {
                    let doc = claude_shared_parse(&sources[0].file.path, bytes.as_deref())?;
                    Ok(claude_import_fields(state)?.into_iter().filter(|(path,_)|claude_path_missing(&doc,path))
                      .map(|(path,value)|serde_json::json!({"field":path.join("."),"path":path,"value":value.to_string()})).collect())
                }
                SharedApp::Gemini => gemini::import_fields(state, &sources),
            }
        };
        Ok(match read() {
            Ok(fields) => serde_json::json!({"app":app.name(),"status":"ready","fields":fields}),
            Err(error) => {
                serde_json::json!({"app":app.name(),"status":"blocked","fields":[],"code":shared_code(&error)})
            }
        })
    }
    fn preview_shared_settings_import(
        app: SharedApp,
        state: &crate::store::AppState,
        fields: &[Vec<String>],
        id: &str,
    ) -> Result<Value, AppError> {
        use crate::live::patch::{toml::TomlPatch, KeyPath, LivePatch};
        let request = shared_request(app, "", id)?;
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.name()));
        let plan = || -> Result<Value, AppError> {
            let _admission =
                crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &app.kind())?;
            let mut sources = app.sources()?;
            if matches!(app, SharedApp::Gemini) {
                let draft = gemini::seed(state, &sources, fields)?;
                let plan = gemini::plan(&draft, id, sources)?;
                let mut value = plan.preview(app);
                value["config"] =
                    serde_json::to_value(draft).map_err(|_| shared_problem("invalidFormat"))?;
                return Ok(value);
            }
            let source = sources.remove(0);
            let file = source.file;
            let pre = source.pre;
            let config = match app {
                SharedApp::Codex => {
                    let full = shared_parse(
                        std::str::from_utf8(pre.as_deref().unwrap_or_default())
                            .map_err(|_| shared_problem("invalidFormat"))?,
                    )?;
                    let source = shared_import_fields(state)?;
                    let mut seed = TomlPatch::default();
                    for path in fields.iter().collect::<std::collections::BTreeSet<_>>() {
                        let item = source
                            .get(path)
                            .ok_or_else(|| shared_problem("readOnlyFields"))?;
                        if shared_path_missing(&full, path) {
                            seed.seed.push((KeyPath(path.clone()), item.clone()));
                        }
                    }
                    let bytes = seed
                        .apply(&file.path, pre.as_deref())
                        .map_err(|_| shared_problem("invalidFormat"))?;
                    app.config_text(Some(&bytes))?
                }
                SharedApp::Claude => {
                    let full = claude_shared_parse(&file.path, pre.as_deref())?;
                    let source = claude_import_fields(state)?;
                    let mut seed = crate::live::patch::json::JsonPatch::default();
                    for path in fields.iter().collect::<std::collections::BTreeSet<_>>() {
                        let item = source
                            .get(path)
                            .ok_or_else(|| shared_problem("readOnlyFields"))?;
                        if claude_path_missing(&full, path) {
                            seed.seed.push((KeyPath(path.clone()), item.clone()));
                        }
                    }
                    let bytes = seed
                        .apply(&file.path, pre.as_deref())
                        .map_err(|_| shared_problem("invalidFormat"))?;
                    app.config_text(Some(&bytes))?
                }
                SharedApp::Gemini => unreachable!(),
            };
            let (request, file, patch, changes) = plan_shared_source(app, &config, id, file, pre)?;
            let mut value = SharedPlan {
                request,
                files: vec![(source.role, file, patch)],
                changes,
            }
            .preview(app);
            value["config"] = serde_json::json!(config);
            Ok(value)
        };
        Ok(match plan() {
            Ok(value) => value,
            Err(error) => {
                serde_json::json!({"app":app.name(),"request":request,"status":"blocked","config":"","changes":[],"files":[],"code":shared_code(&error)})
            }
        })
    }
    fn read_shared_settings(
        app: SharedApp,
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.name()));
        let original = shared_original(app, state)
            .ok()
            .flatten()
            .map(|(request, status)| shared_result(app, &request, status, None));
        let read = || -> Result<SharedDraft, AppError> {
            let _admission =
                crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &app.kind())?;
            shared_draft(app, &app.sources()?)
        };
        Ok(match read() {
            Ok(config) => {
                serde_json::json!({"app":app.name(),"status":"ready","config":config,"originalSave":original})
            }
            Err(error) => {
                serde_json::json!({"app":app.name(),"status":"blocked","config":"","originalSave":original,"code":shared_code(&error)})
            }
        })
    }
    fn preview_shared_settings(
        app: SharedApp,
        state: &crate::store::AppState,
        config: &SharedDraft,
        id: &str,
    ) -> Result<Value, AppError> {
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.name()));
        let request = shared_request(app, &config.encoded()?, id)?;
        Ok(match shared_plan(app, state, config, id) {
            Ok(plan) => plan.preview(app),
            Err(error) => {
                serde_json::json!({"app":app.name(),"request":request,"status":"blocked","changes":[],"files":[],"code":shared_code(&error)})
            }
        })
    }
    fn save_shared_settings(
        app: SharedApp,
        state: &crate::store::AppState,
        config: &SharedDraft,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        use crate::mode::{
            operation::{AppWrite, FileChange},
            state::{op, PendingTarget},
        };
        request.validate()?;
        if request.scope != app.scope()
            || request.draft_digest != crate::live::engine::sha256_hex(config.encoded()?.as_bytes())
        {
            return Ok(shared_result(app, &request, "conflict", None));
        }
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.name()));
        let previous = match shared_query(app, state, &request) {
            Ok(value) => value,
            Err(error) => {
                return Ok(shared_result(
                    app,
                    &request,
                    "blocked",
                    Some(shared_code(&error)),
                ))
            }
        };
        if !matches!(previous["status"].as_str(), Some("notRecorded" | "unknown")) {
            return Ok(previous);
        }
        let plan = match shared_plan(app, state, config, &request.id) {
            Ok(plan) => plan,
            Err(error) => {
                return Ok(shared_result(
                    app,
                    &request,
                    "blocked",
                    Some(shared_code(&error)),
                ))
            }
        };
        if plan.request.revision != request.revision {
            return Ok(shared_result(app, &request, "stale", Some("sourceChanged")));
        }
        let saved = (|| {
            let write = AppWrite::begin_mode(&state.proxy_service, &app.kind())?;
            write.run(
                op::APPLY,
                &plan
                    .files
                    .iter()
                    .map(|(_, file, patch)| FileChange {
                        file: file.clone(),
                        patch,
                    })
                    .collect::<Vec<_>>(),
                PendingTarget {
                    save_request: Some(request.clone()),
                    ..Default::default()
                },
            )
        })();
        let queried = shared_query(app, state, &request)
            .unwrap_or_else(|_| shared_result(app, &request, "unknown", None));
        Ok(match saved {
            Ok(_) => queried,
            Err(error) if matches!(queried["status"].as_str(), Some("notRecorded" | "unknown")) => {
                shared_result(app, &request, "blocked", Some(shared_code(&error)))
            }
            Err(_) => queried,
        })
    }
    fn query_shared_settings_save(
        app: SharedApp,
        state: &crate::store::AppState,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.name()));
        Ok(shared_query(app, state, &request).unwrap_or_else(|error| {
            shared_result(app, &request, "unknown", Some(shared_code(&error)))
        }))
    }
}

impl ConfigService {
    pub(crate) fn read_codex_shared_settings(
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        Self::read_shared_settings(SharedApp::Codex, state)
    }
    pub(crate) fn preview_codex_shared_settings(
        state: &crate::store::AppState,
        config: &str,
        id: &str,
    ) -> Result<Value, AppError> {
        Self::preview_shared_settings(
            SharedApp::Codex,
            state,
            &SharedDraft::Text(config.into()),
            id,
        )
    }
    pub(crate) fn save_codex_shared_settings(
        state: &crate::store::AppState,
        config: &str,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        Self::save_shared_settings(
            SharedApp::Codex,
            state,
            &SharedDraft::Text(config.into()),
            request,
        )
    }
    pub(crate) fn query_codex_shared_settings_save(
        state: &crate::store::AppState,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        Self::query_shared_settings_save(SharedApp::Codex, state, request)
    }
    pub(crate) fn read_codex_shared_settings_import(
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        Self::read_shared_settings_import(SharedApp::Codex, state)
    }
    pub(crate) fn preview_codex_shared_settings_import(
        state: &crate::store::AppState,
        fields: &[Vec<String>],
        id: &str,
    ) -> Result<Value, AppError> {
        Self::preview_shared_settings_import(SharedApp::Codex, state, fields, id)
    }
    pub(crate) fn read_claude_shared_settings(
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        Self::read_shared_settings(SharedApp::Claude, state)
    }
    pub(crate) fn preview_claude_shared_settings(
        state: &crate::store::AppState,
        config: &str,
        id: &str,
    ) -> Result<Value, AppError> {
        Self::preview_shared_settings(
            SharedApp::Claude,
            state,
            &SharedDraft::Text(config.into()),
            id,
        )
    }
    pub(crate) fn save_claude_shared_settings(
        state: &crate::store::AppState,
        config: &str,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        Self::save_shared_settings(
            SharedApp::Claude,
            state,
            &SharedDraft::Text(config.into()),
            request,
        )
    }
    pub(crate) fn query_claude_shared_settings_save(
        state: &crate::store::AppState,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        Self::query_shared_settings_save(SharedApp::Claude, state, request)
    }
    pub(crate) fn read_claude_shared_settings_import(
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        Self::read_shared_settings_import(SharedApp::Claude, state)
    }
    pub(crate) fn preview_claude_shared_settings_import(
        state: &crate::store::AppState,
        fields: &[Vec<String>],
        id: &str,
    ) -> Result<Value, AppError> {
        Self::preview_shared_settings_import(SharedApp::Claude, state, fields, id)
    }
    pub(crate) fn read_gemini_shared_settings(
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        Self::read_shared_settings(SharedApp::Gemini, state)
    }
    pub(crate) fn preview_gemini_shared_settings(
        state: &crate::store::AppState,
        config: &GeminiSharedDraft,
        id: &str,
    ) -> Result<Value, AppError> {
        Self::preview_shared_settings(
            SharedApp::Gemini,
            state,
            &SharedDraft::Gemini(config.clone()),
            id,
        )
    }
    pub(crate) fn save_gemini_shared_settings(
        state: &crate::store::AppState,
        config: &GeminiSharedDraft,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        Self::save_shared_settings(
            SharedApp::Gemini,
            state,
            &SharedDraft::Gemini(config.clone()),
            request,
        )
    }
    pub(crate) fn query_gemini_shared_settings_save(
        state: &crate::store::AppState,
        request: crate::mode::state::SaveRequest,
    ) -> Result<Value, AppError> {
        Self::query_shared_settings_save(SharedApp::Gemini, state, request)
    }
    pub(crate) fn read_gemini_shared_settings_import(
        state: &crate::store::AppState,
    ) -> Result<Value, AppError> {
        Self::read_shared_settings_import(SharedApp::Gemini, state)
    }
    pub(crate) fn preview_gemini_shared_settings_import(
        state: &crate::store::AppState,
        fields: &[Vec<String>],
        id: &str,
    ) -> Result<Value, AppError> {
        Self::preview_shared_settings_import(SharedApp::Gemini, state, fields, id)
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
