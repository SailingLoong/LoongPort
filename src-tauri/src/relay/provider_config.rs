//! Managed client configuration generation, reading and credential updates.
//! Pure transformations reuse upstream builders and never load runtime policy or fetch remote data.

use super::model_selection::{maybe_one_m, ClaudeRoleModels, ModelSelectionTables};
use crate::app_config::AppType;

/// Display name for a site group.
pub fn provider_display_name(site_name: &str, group_name: &str) -> String {
    if site_name.is_empty() {
        group_name.to_string()
    } else {
        format!("{site_name} · {group_name}")
    }
}

/// Generate the Responses configuration; credentials stay in the managed provider.
pub fn codex_config_toml(display_name: &str, base_url: &str, model: &str) -> String {
    codex_config_toml_with_wire(display_name, base_url, model, "responses")
}

/// Generate Codex TOML with an explicit wire format. Preserve the shared custom history bucket.
pub fn codex_config_toml_with_wire(
    display_name: &str,
    base_url: &str,
    model: &str,
    wire_api: &str,
) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    debug_assert!(
        wire_api == "responses" || wire_api == "chat",
        "wire_api 只认 responses / chat：{wire_api}"
    );
    format!(
        r#"model_provider = "custom"
model = {}
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = {}
base_url = {}
wire_api = {}"#,
        q(model),
        q(display_name),
        q(base_url),
        q(wire_api)
    )
}

/// Client configuration differences required by a vendor plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProvisionStyle {
    /// Claude 系鉴权改写 `ANTHROPIC_API_KEY`（x-api-key），**且完全不写**
    /// `ANTHROPIC_AUTH_TOKEN` —— 两个字段同写时 Claude Code 优先 Bearer，被网关
    /// 静默忽略后就是一条必 401 的配置。
    pub claude_auth_via_api_key: bool,
    /// codex 的 `wire_api` 用 `"chat"` 而不是默认 `"responses"`。
    pub codex_wire_chat: bool,
}

/// Generate client settings using the default authentication and model roles.
pub fn settings_config_for(
    app_type: &AppType,
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
) -> Option<serde_json::Value> {
    settings_config_with_roles(app_type, api_key, display_name, base_url, model, None)
}

/// Generate client settings with explicit Claude role assignments.
pub fn settings_config_with_roles(
    app_type: &AppType,
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
    roles: Option<ClaudeRoleModels>,
) -> Option<serde_json::Value> {
    settings_config_with_roles_and_models(
        app_type,
        api_key,
        display_name,
        base_url,
        model,
        roles,
        None,
        ProvisionStyle::default(),
    )
}

/// Generate client settings with the advertised model catalog.
pub fn settings_config_with_models(
    app_type: &AppType,
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
    models: Option<&[String]>,
) -> Option<serde_json::Value> {
    settings_config_with_roles_and_models(
        app_type,
        api_key,
        display_name,
        base_url,
        model,
        None,
        models,
        ProvisionStyle::default(),
    )
}

/// Generate managed settings. Reuse upstream builders except for the Codex bearer-token contract.
#[allow(clippy::too_many_arguments)]
pub fn settings_config_with_roles_and_models(
    app_type: &AppType,
    api_key: &str,
    display_name: &str,
    base_url: &str,
    model: &str,
    roles: Option<ClaudeRoleModels>,
    models: Option<&[String]>,
    style: ProvisionStyle,
) -> Option<serde_json::Value> {
    // codex 例外：上游那份多一行 requires_openai_auth，见上面那段。
    //
    // ⚠️ **生图栏必须与 codex 走同一条**（测试 `the_image_column_shares_the_codex_config_shape`
    // 钉着）：生图 MCP 按 codex 的形状去读 sk 与 base_url。掉进下面那条上游分支会
    // 得到一份 claude/gemini 形状的配置 ⇒ 生图在运行时读不出密钥，而那是只有真机
    // 才发现得了的失败。
    if matches!(app_type, AppType::Codex | AppType::CodexImage) {
        let toml = if style.codex_wire_chat {
            codex_config_toml_with_wire(display_name, base_url, model, "chat")
        } else {
            codex_config_toml(display_name, base_url, model)
        };
        let mut settings = serde_json::json!({
            "auth": { "OPENAI_API_KEY": api_key },
            "config": toml,
        });
        if matches!(app_type, AppType::Codex) {
            if let Some(models) = models.filter(|models| !models.is_empty()) {
                // Mixed groups can advertise `gpt-image-*` alongside chat
                // models. Those belong to the image-generation path and are
                // not valid Codex conversation models, so do not turn them
                // into clickable main-model choices.
                let models = super::model_selection::filter_models(app_type, models);
                if models.is_empty() {
                    return Some(settings);
                }
                settings["modelCatalog"] = serde_json::json!({
                    "models": models
                        .iter()
                        .map(|model| serde_json::json!({ "model": model }))
                        .collect::<Vec<_>>(),
                });
            }
        }
        return Some(settings);
    }

    // 其余交给上游 —— 构造一个等价于「导入到 cc-switch」那个 deeplink 的请求。
    let request = crate::deeplink::DeepLinkImportRequest {
        version: "v1".to_string(),
        resource: "provider".to_string(),
        app: Some(app_type.as_str().to_string()),
        name: Some(display_name.to_string()),
        endpoint: Some(base_url.to_string()),
        api_key: Some(api_key.to_string()),
        model: Some(model.to_string()),
        // ⚠️ **别名必须显式给**：上游只在请求里带了才写这几个 env
        // （`build_claude_settings` 的 `if let Some(haiku_model)`）。不给的话
        // Claude Code 会按 haiku/sonnet/opus 各自的默认名去请求，而中转站那边
        // 通常只认一个模型名 ⇒ 用户切到 sonnet 就报「模型不存在」。
        //
        // 默认（`roles = None`）全部指向同一个 model：中转站的分组是「一个 sk
        // 一档价」，没有「便宜的 haiku、贵的 opus」这种分层，硬分会让用户以为能选。
        // 官网直连例外 —— 见 [`ClaudeRoleModels`]。
        haiku_model: Some(
            roles
                .as_ref()
                .map_or(model, |r| r.haiku.as_str())
                .to_string(),
        ),
        sonnet_model: Some(
            roles
                .as_ref()
                .map_or(model, |r| r.sonnet.as_str())
                .to_string(),
        ),
        opus_model: Some(
            roles
                .as_ref()
                .map_or(model, |r| r.opus.as_str())
                .to_string(),
        ),
        // 这两个**只在分档时写**：`roles = None`（中转站）那条路保持原样，
        // 不给已有档位凭空多两个键 —— 那会让全部存量档位的整份比对失配，
        // 集体误报「已手工维护」。
        fable_model: roles.as_ref().map(|r| r.fable.clone()),
        subagent_model: roles.as_ref().map(|r| r.subagent.clone()),
        // Claude 系鉴权字段的选择（Go 网关只认 x-api-key）。None = 默认 Bearer。
        claude_api_key_auth: style.claude_auth_via_api_key.then_some(true),
        homepage: None,
        ..Default::default()
    };

    crate::deeplink::build_provider_from_request(app_type, &request)
        .ok()
        .map(|p| {
            let mut config = p.settings_config;
            if matches!(app_type, AppType::Claude) {
                config["language"] = serde_json::json!("chinese");
            }
            // 把分组模型目录落进配置（主界面模型芯片与自动模式 app→模型 映射的
            // 数据源，形状与 codex 的 modelCatalog 相同）。**全部非 codex 平台**：
            // 目录是档位级事实（「这把 key 能服务哪些模型」），省心模式的偏好过滤
            // 对无目录档位是静默排除 —— 2026-08-17 真实 smoke 实测官网直连的
            // hermes/openclaw/opencode 行中招。与 codex 的 `is_image_model` 过滤
            // 同理按家族收口：
            // - gemini 走原生 URL 路由（`/v1beta/models/{model}:generateContent`），
            //   只认 gemini-* 家族，目录只收 gemini-*；
            // - 其余平台目录收全部**文本**模型（claude 分组可能混 gpt-* / deepseek-*，
            //   跨家族角色对齐，角色挑选本来就跨家族）。
            if let Some(models) = models.filter(|models| !models.is_empty()) {
                let family = super::model_selection::filter_models(app_type, models);
                if !family.is_empty() {
                    config["modelCatalog"] = serde_json::json!({
                        "models": family
                            .iter()
                            .map(|model| serde_json::json!({ "model": model }))
                            .collect::<Vec<_>>(),
                    });
                }
            }
            config
        })
}

/// Read the model from a generated Codex configuration.
pub fn extract_model(settings_config: &serde_json::Value) -> Option<String> {
    let config = settings_config.get("config")?.as_str()?;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        // 严格相等：`model_provider` / `model_reasoning_effort` 都以 `model` 开头。
        if lhs.trim() != "model" {
            continue;
        }
        let value = rhs.trim();
        let unquoted = value.strip_prefix('"')?.strip_suffix('"')?;
        if unquoted.is_empty() {
            return None;
        }
        return Some(unquoted.to_string());
    }
    None
}

/// Clients supporting managed model selection. Consumers must use this list.
pub fn model_catalog_apps() -> &'static [AppType] {
    &[
        AppType::Claude,
        AppType::Codex,
        AppType::Gemini,
        AppType::GrokBuild,
    ]
}

/// Whether the client supports managed model selection.
pub fn supports_model_catalog(app_type: &AppType) -> bool {
    model_catalog_apps().contains(app_type)
}

/// Read the selected model using the client configuration format.
pub fn selected_model(app_type: &AppType, settings: &serde_json::Value) -> Option<String> {
    let env_key = match app_type {
        AppType::Codex | AppType::CodexImage => return extract_model(settings),
        AppType::GrokBuild => {
            let config = settings.get("config")?.as_str()?;
            return crate::grok_config::extract_model_config(config).map(|c| c.model);
        }
        AppType::Claude => "ANTHROPIC_MODEL",
        AppType::Gemini => "GEMINI_MODEL",
        _ => return None,
    };
    settings
        .pointer(&format!("/env/{env_key}"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

/// Change only the selected env model, using the supplied context-capability rules.
pub fn select_env_model(
    app_type: &AppType,
    settings: &serde_json::Value,
    model: &str,
    tables: &ModelSelectionTables,
) -> Result<serde_json::Value, crate::error::AppError> {
    let env_key = match app_type {
        AppType::Claude => "ANTHROPIC_MODEL",
        AppType::Gemini => "GEMINI_MODEL",
        other => {
            return Err(crate::error::AppError::Config(format!(
                "{} 档位不支持选模型",
                other.as_str()
            )))
        }
    };
    let mut config = settings.clone();
    let Some(env) = config.get_mut("env").and_then(|env| env.as_object_mut()) else {
        return Err(crate::error::AppError::Config(format!(
            "配置里缺少 env 对象，无法写入 {env_key}"
        )));
    };
    // claude 平台选模型写入时与 provision 同一张 1M 前缀表（远端合并后的）——
    // 否则远端更新名单后，用户一切模型就丢掉新声明。
    let value = if matches!(app_type, AppType::Claude) {
        maybe_one_m(tables, model)
    } else {
        model.to_string()
    };
    env.insert(env_key.to_string(), serde_json::json!(value));
    Ok(config)
}

/// Retain a user selection while it remains in the refreshed catalog.
pub fn preserve_supported_env_model(
    app_type: &AppType,
    defaults: serde_json::Value,
    previous: &serde_json::Value,
    catalog: &[String],
    tables: &ModelSelectionTables,
) -> serde_json::Value {
    let Some(old) = selected_model(app_type, previous) else {
        return defaults;
    };
    let bare = old.trim_end_matches(crate::claude_desktop_config::ONE_M_CONTEXT_MARKER);
    if !catalog.iter().any(|m| m == bare) {
        return defaults;
    }
    select_env_model(app_type, &defaults, bare, tables).unwrap_or(defaults)
}

/// Read nonempty model identifiers from the stored model catalog.
pub(crate) fn models_from_settings(settings: &serde_json::Value) -> Vec<String> {
    settings
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("model").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .collect()
}

fn api_key_locations(app_type: &AppType) -> Option<&'static [&'static [&'static str]]> {
    const CODEX: &[&str] = &["auth", "OPENAI_API_KEY"];
    const CLAUDE_AUTH_TOKEN: &[&str] = &["env", "ANTHROPIC_AUTH_TOKEN"];
    const CLAUDE_API_KEY: &[&str] = &["env", "ANTHROPIC_API_KEY"];
    const GEMINI: &[&str] = &["env", "GEMINI_API_KEY"];
    const HERMES: &[&str] = &["api_key"];
    const OPENCLAW: &[&str] = &["apiKey"];
    const OPENCODE: &[&str] = &["options", "apiKey"];

    match app_type {
        AppType::Codex | AppType::CodexImage => Some(&[CODEX]),
        AppType::Claude | AppType::ClaudeDesktop => Some(&[CLAUDE_AUTH_TOKEN, CLAUDE_API_KEY]),
        AppType::Gemini => Some(&[GEMINI]),
        AppType::Hermes => Some(&[HERMES]),
        AppType::OpenClaw => Some(&[OPENCLAW]),
        AppType::OpenCode => Some(&[OPENCODE]),
        // Grok Build 的 sk 在 `config` 字段的 TOML 文本内部（`[model."<default>"]` 表的
        // `api_key`），JSON 路径表达不了 ⇒ 走 [`extract_api_key`] / [`patch_api_key`]
        // 开头的 TOML 专用分支，这里归 None。
        AppType::GrokBuild => None,
        // Pi 的 provider 形态与中转站链路无关：归 None，不参与 sk 提取/判等。
        AppType::Pi => None,
    }
}

fn value_at_path<'a>(root: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    path.iter().try_fold(root, |value, key| value.get(*key))
}

fn object_at_parent_path<'a>(
    root: &'a mut serde_json::Value,
    path: &[&str],
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let (_, parent) = path.split_last()?;
    let parent = parent
        .iter()
        .try_fold(root, |value, key| value.get_mut(*key))?;
    parent.as_object_mut()
}

fn ensure_object_at_parent_path<'a>(
    root: &'a mut serde_json::Value,
    path: &[&str],
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let (_, parent) = path.split_last()?;
    let mut current = root.as_object_mut()?;
    for key in parent {
        let value = current
            .entry((*key).to_string())
            .or_insert_with(|| serde_json::json!({}));
        current = value.as_object_mut()?;
    }
    Some(current)
}

fn grok_config_text(settings_config: &serde_json::Value) -> Option<&str> {
    settings_config.get("config")?.as_str()
}

/// Read the managed credential using the client configuration format.
pub fn extract_api_key(settings_config: &serde_json::Value, app_type: &AppType) -> Option<String> {
    if matches!(app_type, AppType::GrokBuild) {
        return grok_config_text(settings_config)
            .and_then(crate::grok_config::extract_inline_api_key);
    }
    api_key_locations(app_type)?.iter().find_map(|path| {
        value_at_path(settings_config, path)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

/// Replace the credential in an existing configuration while preserving user edits.
pub fn patch_api_key(
    settings_config: &mut serde_json::Value,
    app_type: &AppType,
    api_key: &str,
) -> bool {
    // Grok Build：改写 TOML 文本后写回 `config` 字段（toml_edit 保格式保注释，
    // 用户的手工编辑不丢）。形状坏到改不动时返回 false，走调用方的「全量重写」回落。
    // 字段被删掉时 `update_api_key` 会补回 —— ensure 语义（编辑器丢认证字段后
    // 把托管凭据补回去）与 patch 语义在这里天然合并，无需两套。
    if matches!(app_type, AppType::GrokBuild) {
        let Some(text) = grok_config_text(settings_config) else {
            return false;
        };
        let Ok(updated) = crate::grok_config::update_api_key(text, api_key) else {
            return false;
        };
        // `grok_config_text` 拿到了 `config` 字符串 ⇒ 根必是对象，这里直接写回。
        settings_config["config"] = serde_json::Value::String(updated);
        return true;
    }

    let Some(locations) = api_key_locations(app_type) else {
        return false;
    };

    // 所有已经存在的候选字段都改成同一把 key。Claude 配置若意外同时含两种字段，
    // 只改一个会让运行时与倍率查询各读到不同的凭据。
    let mut patched = false;
    for path in locations {
        if let Some(map) = object_at_parent_path(settings_config, path) {
            let field = *path.last().expect("API key path is not empty");
            if map.contains_key(field) {
                map.insert(field.to_string(), serde_json::json!(api_key));
                patched = true;
            }
        }
    }
    if patched {
        return true;
    }

    // section 存在但 key 被用户删掉时，补回默认字段，避免下一次倍率查询丢凭据。
    let Some(map) = object_at_parent_path(settings_config, locations[0]) else {
        return false;
    };
    let field = *locations[0].last().expect("API key path is not empty");
    map.insert(field.to_string(), serde_json::json!(api_key));
    true
}

/// Restore missing credential sections after editing. Reject incompatible configuration shapes.
pub fn ensure_api_key(
    settings_config: &mut serde_json::Value,
    app_type: &AppType,
    api_key: &str,
) -> bool {
    if patch_api_key(settings_config, app_type, api_key) {
        return true;
    }

    let Some(path) = api_key_locations(app_type).and_then(|locations| locations.first().copied())
    else {
        return false;
    };
    let Some(map) = ensure_object_at_parent_path(settings_config, path) else {
        return false;
    };
    let field = *path.last().expect("API key path is not empty");
    map.insert(field.to_string(), serde_json::json!(api_key));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::model_selection::pick_tier_models;
    fn models(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_image_column_shares_the_codex_config_shape() {
        let base = "https://api.x.example/v1";
        let codex = settings_config_for(&AppType::Codex, "sk-1", "档", base, "gpt-image-2")
            .expect("codex 必须有形状");
        let image = settings_config_for(&AppType::CodexImage, "sk-1", "档", base, "gpt-image-2")
            .expect("生图栏必须有形状");
        assert_eq!(
            codex, image,
            "生图栏的配置形状与 codex 分叉了 —— 生图 MCP 会读不出 sk"
        );
    }

    #[test]
    fn extract_model_matches_the_whole_key_not_a_prefix() {
        let cfg = settings_config_for(
            &AppType::Codex,
            "sk",
            "t",
            "https://x.example/v1",
            "gpt-image-2",
        )
        .expect("codex 必须有默认形状");
        assert_eq!(extract_model(&cfg).as_deref(), Some("gpt-image-2"));
    }

    #[test]
    fn claude_tier_flows_into_generated_settings_config() {
        let list = models(&[
            "claude-fable-5",
            "claude-haiku-4-5",
            "claude-opus-5",
            "claude-sonnet-5",
        ]);
        let picked = pick_tier_models(&AppType::Claude, Some(&list));
        let roles = picked.claude_roles.expect("claude 必须有角色模型");
        let cfg = settings_config_with_roles(
            &AppType::Claude,
            "sk-1",
            "示例内部 api · Anthropic 模型-导入 Claude Code",
            "https://other-relay.example/v1",
            &picked.main,
            Some(roles),
        )
        .expect("claude 必须有形状");
        let env = &cfg["env"];
        // 支持 1M 的 claude 新一代模型自动带 `[1m]` 后缀声明（转发时剥掉）。
        assert_eq!(env["ANTHROPIC_MODEL"], "claude-opus-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "claude-opus-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_FABLE_MODEL"], "claude-opus-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_SONNET_MODEL"], "claude-sonnet-5[1m]");
        assert_eq!(env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-haiku-4-5[1m]");
        // 修复前这里是 gpt-5.6-sol —— 模型列表明明全 claude，档位却写 openai 模型。
        assert_ne!(env["ANTHROPIC_MODEL"], "gpt-5.6-sol");
    }

    #[test]
    fn config_toml_uses_custom_provider_id_never_openai() {
        let toml = codex_config_toml(
            "Example Relay · Pro",
            "https://relay.example/v1",
            "gpt-5.6-sol",
        );
        assert!(toml.contains(r#"model_provider = "custom""#));
        assert!(toml.contains("[model_providers.custom]"));
        // 这条钉住那个陷阱：sub2api 面板模板写的是 "OpenAI"，照抄会让 token 落到顶层
        // 且会话桶分家。
        assert!(!toml.contains("OpenAI\""), "{toml}");
        assert!(!toml.contains("[model_providers.OpenAI]"), "{toml}");
    }

    #[test]
    fn config_toml_has_the_mandatory_flags() {
        let toml = codex_config_toml("n", "https://x.example/v1", "m");
        // 漏 disable_response_storage → codex 发 previous_response_id → sub2api 直接 400。
        assert!(toml.contains("disable_response_storage = true"));
        // sub2api 的 openai 网关原生走 responses，chat 是错的。
        assert!(toml.contains(r#"wire_api = "responses""#));
    }

    #[test]
    fn codex_settings_persist_the_fetched_model_catalog() {
        let models = vec![
            "gpt-a".to_string(),
            "gpt-b".to_string(),
            "gpt-image-2".to_string(),
        ];
        let settings = settings_config_with_models(
            &AppType::Codex,
            "sk-test",
            "Test",
            "https://api.example.com/v1",
            "gpt-a",
            Some(&models),
        )
        .expect("Codex config");

        assert_eq!(
            settings["modelCatalog"]["models"],
            serde_json::json!([
                { "model": "gpt-a" },
                { "model": "gpt-b" }
            ])
        );

        let image_settings = settings_config_with_models(
            &AppType::CodexImage,
            "sk-test",
            "Image",
            "https://api.example.com/v1",
            "gpt-image-2",
            Some(&models),
        )
        .expect("Codex image config");
        assert!(
            image_settings.get("modelCatalog").is_none(),
            "生图栏不消费 Codex 主模型目录"
        );
    }

    #[test]
    fn claude_settings_persist_the_text_model_catalog() {
        let models = vec![
            "claude-opus-5".to_string(),
            "gpt-5.6-sol".to_string(),
            "gpt-image-2".to_string(),
        ];
        let settings = settings_config_with_roles_and_models(
            &AppType::Claude,
            "sk-test",
            "Test",
            "https://api.example.com/v1",
            "claude-opus-5",
            None,
            Some(&models),
            ProvisionStyle::default(),
        )
        .expect("Claude config");

        assert_eq!(
            settings["modelCatalog"]["models"],
            serde_json::json!([
                { "model": "claude-opus-5" },
                { "model": "gpt-5.6-sol" }
            ])
        );
        // 选中模型在 env 里可读（`[1m]` 后缀是 pick_tier_models 挑选时加的，
        // 这里传什么写什么 —— 裸 id 进、裸 id 出）
        assert_eq!(
            selected_model(&AppType::Claude, &settings).as_deref(),
            Some("claude-opus-5")
        );
    }

    #[test]
    fn gemini_settings_persist_the_gemini_family_catalog_only() {
        let mixed = vec![
            "gemini-3-pro".to_string(),
            "claude-opus-5".to_string(),
            "gpt-5.6-sol".to_string(),
        ];
        let settings = settings_config_with_roles_and_models(
            &AppType::Gemini,
            "sk-test",
            "Test",
            "https://api.example.com/v1",
            "gemini-3-pro",
            None,
            Some(&mixed),
            ProvisionStyle::default(),
        )
        .expect("Gemini config");
        assert_eq!(
            settings["modelCatalog"]["models"],
            serde_json::json!([{ "model": "gemini-3-pro" }])
        );
        assert_eq!(
            selected_model(&AppType::Gemini, &settings).as_deref(),
            Some("gemini-3-pro")
        );

        let no_family = vec!["claude-opus-5".to_string(), "gpt-5.6-sol".to_string()];
        let settings = settings_config_with_roles_and_models(
            &AppType::Gemini,
            "sk-test",
            "Test",
            "https://api.example.com/v1",
            "claude-opus-5",
            None,
            Some(&no_family),
            ProvisionStyle::default(),
        )
        .expect("Gemini config");
        assert!(
            settings.get("modelCatalog").is_none(),
            "没有 gemini-* 时不能写一个空目录"
        );
    }

    #[test]
    fn grok_settings_persist_the_text_model_catalog() {
        let models = vec![
            "grok-4.5".to_string(),
            "grok-code-4.5".to_string(),
            "gpt-image-2".to_string(),
        ];
        let settings = settings_config_with_models(
            &AppType::GrokBuild,
            "sk-test",
            "Test",
            "https://api.example.com",
            "grok-4.5",
            Some(&models),
        )
        .expect("Grok config");

        assert_eq!(
            settings["modelCatalog"]["models"],
            serde_json::json!([
                { "model": "grok-4.5" },
                { "model": "grok-code-4.5" }
            ])
        );
        assert_eq!(
            selected_model(&AppType::GrokBuild, &settings).as_deref(),
            Some("grok-4.5")
        );
    }

    #[test]
    fn model_edits_use_the_supplied_capability_snapshot() {
        let mut tables = ModelSelectionTables::builtin();
        tables.one_m_prefixes = vec!["custom-text-".into()];
        let defaults = serde_json::json!({"env": {"ANTHROPIC_MODEL": "default", "ANTHROPIC_AUTH_TOKEN": "sk-test"}});
        let selected =
            select_env_model(&AppType::Claude, &defaults, "custom-text-v1", &tables).unwrap();
        assert_eq!(selected["env"]["ANTHROPIC_MODEL"], "custom-text-v1[1m]");
        assert_eq!(selected["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-test");
        let preserved = preserve_supported_env_model(
            &AppType::Claude,
            defaults.clone(),
            &selected,
            &models(&["custom-text-v1"]),
            &tables,
        );
        assert_eq!(preserved, selected);
        tables.one_m_prefixes.clear();
        let selected =
            select_env_model(&AppType::Claude, &defaults, "custom-text-v1", &tables).unwrap();
        assert_eq!(selected["env"]["ANTHROPIC_MODEL"], "custom-text-v1");
    }

    #[test]
    fn env_model_selection_and_preserve() {
        let models = vec![
            "claude-sonnet-5".to_string(),
            "claude-haiku-4-5".to_string(),
        ];
        let settings = settings_config_with_roles_and_models(
            &AppType::Claude,
            "sk-test",
            "Test",
            "https://api.example.com/v1",
            "claude-sonnet-5",
            None,
            Some(&models),
            ProvisionStyle::default(),
        )
        .expect("Claude config");

        let switched = select_env_model(
            &AppType::Claude,
            &settings,
            "claude-haiku-4-5",
            &ModelSelectionTables::builtin(),
        )
        .expect("select env model");
        assert_eq!(
            selected_model(&AppType::Claude, &switched).as_deref(),
            Some("claude-haiku-4-5[1m]")
        );

        // 保留偏好：旧选中值（带 [1M] 声明）在新目录里 → 新默认的选中键改回旧值
        let defaults = settings.clone();
        let preserved = preserve_supported_env_model(
            &AppType::Claude,
            defaults.clone(),
            &switched,
            &models,
            &ModelSelectionTables::builtin(),
        );
        assert_eq!(
            selected_model(&AppType::Claude, &preserved).as_deref(),
            Some("claude-haiku-4-5[1m]")
        );

        // 上游下架：目录里没有旧值 → 新默认接管
        let shrunk = vec!["claude-sonnet-5".to_string()];
        let reset = preserve_supported_env_model(
            &AppType::Claude,
            defaults,
            &switched,
            &shrunk,
            &ModelSelectionTables::builtin(),
        );
        // 新默认接管：生成侧写入的是裸 id（[1m] 只在 pick/选模型时补）
        assert_eq!(
            selected_model(&AppType::Claude, &reset).as_deref(),
            Some("claude-sonnet-5")
        );
    }

    #[test]
    fn config_toml_must_not_declare_requires_openai_auth() {
        // 这条是 `codex doctor` 实测出来的，方向与上游预设**相反**，所以特别容易被
        // 「照抄上游模板」改回去。
        //
        // LoongPort 把 sk 放在 config.toml 的 experimental_bearer_token 里、不碰 auth.json。
        // 那种情况下声明 requires_openai_auth 会让 codex 判成 ChatGPT 登录模式，去打
        // chatgpt.com/backend-api 拿 403 并报 credentials incomplete —— 实测 1 fail。
        // 删掉它才走 provider auth 打中转站的 /v1（实测 0 fail）。
        let toml = codex_config_toml("n", "https://x.example/v1", "m");
        assert!(
            !toml.contains("requires_openai_auth"),
            "声明了 requires_openai_auth 会让 codex 去打 chatgpt.com 而不是中转站: {toml}"
        );
    }

    #[test]
    fn our_codex_toml_is_upstreams_minus_exactly_one_line() {
        const DISPLAY: &str = "Pro tier";
        const BASE_URL: &str = "https://ops.example.example/v1";
        const MODEL: &str = "gpt-5-codex";

        // 上游那份 —— 走它自己的入口，不复制它的代码。
        let request = crate::deeplink::DeepLinkImportRequest {
            version: "v1".to_string(),
            resource: "provider".to_string(),
            app: Some("codex".to_string()),
            name: Some(DISPLAY.to_string()),
            endpoint: Some(BASE_URL.to_string()),
            api_key: Some("sk-test".to_string()),
            model: Some(MODEL.to_string()),
            ..Default::default()
        };
        let upstream = crate::deeplink::build_provider_from_request(&AppType::Codex, &request)
            .expect("上游必须能构造 codex provider");
        let upstream_toml = upstream.settings_config["config"]
            .as_str()
            .expect("上游那份要有 config 字符串");

        let ours = codex_config_toml(DISPLAY, BASE_URL, MODEL);

        // 逐行比：上游的行去掉 `requires_openai_auth` 那一行之后，必须与我们的逐行相同。
        let upstream_lines: Vec<&str> = upstream_toml
            .lines()
            .filter(|l| !l.contains("requires_openai_auth"))
            .map(|l| l.trim_end())
            .collect();
        let our_lines: Vec<&str> = ours.lines().map(|l| l.trim_end()).collect();

        // 上游那份结尾多一个换行（raw string 里带了），我们的没有 —— 掐掉尾随空行再比，
        // 免得这道闸被一个尾随空行绊住（那不是漂移）。
        fn strip_trailing_blanks(mut v: Vec<&str>) -> Vec<&str> {
            while v.last().is_some_and(|l| l.is_empty()) {
                v.pop();
            }
            v
        }
        let upstream_lines = strip_trailing_blanks(upstream_lines);
        let our_lines = strip_trailing_blanks(our_lines);

        assert_eq!(
            our_lines,
            upstream_lines,
            "codex 模板与上游漂移了。\n  \
             我们的:\n{ours}\n  \
             上游的（已滤掉 requires_openai_auth 行）:\n{}\n  \
             —— 要么上游加了新键我们没跟上（那要判断是否该跟），\
             要么我们改了不该改的地方。有意偏离请在这里写清并调整断言。",
            upstream_lines.join("\n")
        );

        // 顺带钉住「差异恰好是那一行」这件事本身：上游哪天自己删了它，
        // 我们这个例外就没有存在理由了，该收到通知。
        assert!(
            upstream_toml.contains("requires_openai_auth = true"),
            "上游那份不再声明 requires_openai_auth —— 我们那条例外的前提消失了，\
             可以直接复用上游模板，去掉 provision.rs 里这段手抄的副本"
        );
    }

    #[test]
    fn config_toml_quotes_values_so_names_cannot_break_toml() {
        // 分组名来自服务端，含引号或反斜杠时不转义就会写出坏 TOML，切换时解析失败。
        let toml = codex_config_toml(r#"Pro "special" \ tier"#, "https://x.example/v1", "m");
        let parsed: toml::Table = toml.parse().expect("生成的 TOML 必须可解析");
        assert_eq!(
            parsed["model_providers"]["custom"]["name"]
                .as_str()
                .unwrap(),
            r#"Pro "special" \ tier"#
        );
    }

    #[test]
    fn settings_config_always_carries_the_auth_key() {
        // auth 键缺失会让 write_live_snapshot 的 Codex 分支直接报错。
        let sc = settings_config_for(&AppType::Codex, "sk-abc", "n", "https://x.example/v1", "m")
            .expect("codex 必须有形状");
        assert_eq!(sc["auth"]["OPENAI_API_KEY"].as_str().unwrap(), "sk-abc");
        assert!(sc["config"].as_str().unwrap().contains("model_provider"));
    }

    #[test]
    fn patch_api_key_replaces_only_the_key_and_keeps_user_edits() {
        // 用户编辑过的配置：改了模型、加了自定义字段。
        let mut sc =
            settings_config_for(&AppType::Codex, "sk-old", "n", "https://x.example/v1", "m")
                .expect("codex 必须有形状");
        sc["config"] = serde_json::json!("model = \"用户改过的模型\"\n自定义 = 1");
        sc["auth"]["用户加的字段"] = serde_json::json!("保留我");

        assert!(patch_api_key(&mut sc, &AppType::Codex, "sk-new"));

        // sk 换了。
        assert_eq!(sc["auth"]["OPENAI_API_KEY"], "sk-new");
        // **用户的编辑必须还在** —— 这条是这个函数存在的全部理由：
        // 重复 provision 走全量覆盖会把它们冲掉，而用户点「获取密钥」通常只想刷新列表。
        assert_eq!(sc["config"], "model = \"用户改过的模型\"\n自定义 = 1");
        assert_eq!(sc["auth"]["用户加的字段"], "保留我");
    }

    #[test]
    fn claude_api_key_field_is_supported_by_read_and_patch() {
        let mut sc = serde_json::json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-old",
                "ANTHROPIC_MODEL": "用户改过的模型"
            }
        });

        assert_eq!(
            extract_api_key(&sc, &AppType::Claude).as_deref(),
            Some("sk-old")
        );
        assert!(patch_api_key(&mut sc, &AppType::Claude, "sk-new"));
        assert_eq!(sc["env"]["ANTHROPIC_API_KEY"], "sk-new");
        assert!(sc["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
        assert_eq!(sc["env"]["ANTHROPIC_MODEL"], "用户改过的模型");

        sc["env"]["ANTHROPIC_AUTH_TOKEN"] = serde_json::json!("sk-stale");
        assert!(patch_api_key(&mut sc, &AppType::Claude, "sk-unified"));
        assert_eq!(sc["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-unified");
        assert_eq!(sc["env"]["ANTHROPIC_API_KEY"], "sk-unified");
    }

    #[test]
    fn ensure_api_key_recreates_a_missing_auth_section() {
        let mut sc = serde_json::json!({
            "config": "model = \"用户改过的模型\""
        });

        assert!(ensure_api_key(&mut sc, &AppType::Codex, "sk-managed"));
        assert_eq!(sc["auth"]["OPENAI_API_KEY"], "sk-managed");
        assert_eq!(sc["config"], "model = \"用户改过的模型\"");
    }

    #[test]
    fn grokbuild_api_key_round_trips_through_the_toml_branch() {
        let sc = settings_config_for(
            &AppType::GrokBuild,
            "sk-old",
            "n",
            "https://g.example/v1",
            "grok-4.5",
        )
        .expect("grokbuild 必须有形状");
        assert_eq!(
            extract_api_key(&sc, &AppType::GrokBuild).as_deref(),
            Some("sk-old")
        );

        // env_key 形状不认：凭据在进程环境变量里，读了指纹就会随运行环境漂，
        // 也和 JSON 路径机制「只读配置里写着的字段」的语义不对齐。
        let env_only = serde_json::json!({
            "config": "[models]\ndefault = \"p\"\n\n[model.p]\nmodel = \"m\"\nbase_url = \"https://g.example/v1\"\nname = \"n\"\nenv_key = \"GROK_SK\"\napi_backend = \"openai-compliant\"\ncontext_window = 1000000\n"
        });
        assert_eq!(extract_api_key(&env_only, &AppType::GrokBuild), None);
    }

    #[test]
    fn patch_api_key_updates_grokbuild_toml_and_keeps_user_edits() {
        let mut sc = settings_config_for(
            &AppType::GrokBuild,
            "sk-old",
            "n",
            "https://g.example/v1",
            "grok-4.5",
        )
        .expect("grokbuild 必须有形状");
        // 用户编辑过的配置：加了注释与自定义字段（中文键必须带引号 —— TOML 裸键
        // 只允许 ASCII）。config 文本以选中模型表结尾，追加的行落进同一张表；
        // toml_edit 保格式改写，patch 后这些必须还在。
        let with_edits = format!(
            "{}# 用户注释\n\"自定义\" = 1\n",
            sc["config"].as_str().unwrap()
        );
        sc["config"] = serde_json::json!(with_edits);

        assert!(patch_api_key(&mut sc, &AppType::GrokBuild, "sk-new"));
        let toml_text = sc["config"].as_str().unwrap();
        assert!(toml_text.contains("api_key = \"sk-new\""));
        assert!(!toml_text.contains("sk-old"));
        assert!(toml_text.contains("# 用户注释"));
        assert!(toml_text.contains("\"自定义\" = 1"));
    }

    #[test]
    fn ensure_api_key_recreates_a_deleted_grokbuild_key() {
        let mut sc = settings_config_for(
            &AppType::GrokBuild,
            "sk-managed",
            "n",
            "https://g.example/v1",
            "grok-4.5",
        )
        .expect("grokbuild 必须有形状");
        // 编辑器把 api_key 字段删了：选中模型表还在，ensure 应把托管凭据补回。
        let toml_text = sc["config"]
            .as_str()
            .unwrap()
            .replace("api_key = \"sk-managed\"\n", "");
        assert!(!toml_text.contains("sk-managed"));
        sc["config"] = serde_json::json!(toml_text);

        assert!(ensure_api_key(&mut sc, &AppType::GrokBuild, "sk-managed"));
        assert!(sc["config"]
            .as_str()
            .unwrap()
            .contains("api_key = \"sk-managed\""));
    }

    #[test]
    fn patch_api_key_refuses_broken_grokbuild_shapes_instead_of_inventing_one() {
        let mut broken = serde_json::json!({"config": "not toml {"});
        assert!(!patch_api_key(&mut broken, &AppType::GrokBuild, "sk"));

        // 没有 models.default ⇒ 找不到选中模型表，改不动就是改不动，
        // 原文必须原样保留，交给调用方走「全量重写」回落。
        let mut no_default = serde_json::json!({"config": "[model.x]\napi_key = \"k\"\n"});
        assert!(!patch_api_key(&mut no_default, &AppType::GrokBuild, "sk"));
        assert_eq!(no_default["config"], "[model.x]\napi_key = \"k\"\n");
    }

    #[test]
    fn claude_default_carries_language_chinese_but_desktop_does_not() {
        let claude =
            settings_config_for(&AppType::Claude, "sk-1", "n", "https://x.example/v1", "m")
                .expect("claude 必须有形状");
        assert_eq!(
            claude["language"], "chinese",
            "Claude Code 默认配置该带 language: chinese"
        );

        let desktop = settings_config_for(
            &AppType::ClaudeDesktop,
            "sk-1",
            "n",
            "https://x.example/v1",
            "m",
        )
        .expect("claude-desktop 必须有形状");
        assert!(
            desktop.get("language").is_none(),
            "Claude Desktop 不带 language —— 维护者指定只在 claudecode"
        );
    }

    #[test]
    fn generating_the_same_config_twice_yields_identical_json() {
        for app in [AppType::Codex, AppType::Claude, AppType::Gemini] {
            let make = || {
                settings_config_for(&app, "sk-1", "名字", "https://x.example/v1", "m")
                    .unwrap_or_else(|| panic!("{} 应该有形状", app.as_str()))
            };
            let (a, b, c) = (make(), make(), make());
            assert_eq!(a, b, "{} 的默认配置两次生成不一致", app.as_str());
            assert_eq!(b, c, "{} 的默认配置三次生成不一致", app.as_str());

            // 序列化后的**字节**也要一致 —— `Value` 相等但键序不同的话，
            // 比对本身没问题（`Value` 的 Eq 不看 Map 顺序），但落库再读回来
            // 会经过一轮字符串往返，那时顺序就参与了。
            assert_eq!(
                serde_json::to_string(&a).expect("可序列化"),
                serde_json::to_string(&c).expect("可序列化"),
                "{} 的默认配置序列化后不稳定（键序在变？）",
                app.as_str()
            );
        }
    }

    #[test]
    fn patch_api_key_refuses_broken_shapes_instead_of_inventing_one() {
        // 该放 sk 的 section 不见了（用户改坏了）⇒ 返回 false 让调用方全量重写。
        // **不能凭空造一个 auth 段** —— 那会拼出半新半旧的配置，比重写更难查。
        let mut no_auth = serde_json::json!({ "config": "model = \"m\"" });
        assert!(!patch_api_key(&mut no_auth, &AppType::Codex, "sk-new"));
        assert!(no_auth.get("auth").is_none(), "不该凭空造出 auth 段");

        // section 存在但不是对象。
        let mut wrong_type = serde_json::json!({ "auth": "不是对象" });
        assert!(!patch_api_key(&mut wrong_type, &AppType::Codex, "sk-new"));

        // 还没接的 CLI。
        let mut sc = serde_json::json!({ "env": {} });
        assert!(!patch_api_key(&mut sc, &AppType::OpenCode, "sk-new"));
    }

    #[test]
    fn extract_api_key_round_trips_for_every_supported_cli() {
        // 「恢复默认」要先把 sk 读出来再塞回去 —— 读写必须认同一个字段。
        // 两处各写一遍字段名迟早分叉，所以它们共用 api_key_location；这条测试守住往返。
        for app_type in [
            AppType::Codex,
            AppType::Claude,
            AppType::Gemini,
            AppType::Hermes,
            AppType::OpenClaw,
            AppType::OpenCode,
        ] {
            let sc = settings_config_for(&app_type, "sk-abc", "n", "https://x.example/v1", "m")
                .unwrap_or_else(|| panic!("{app_type:?} 必须有形状"));
            assert_eq!(
                extract_api_key(&sc, &app_type).as_deref(),
                Some("sk-abc"),
                "{app_type:?} 的 sk 写进去又读不出来 —— patch 与 extract 的字段对不上了"
            );
        }

        // 空 sk 当作「没有」：一份 sk 为空串的配置恢复默认后仍然不可用，
        // 该让调用方报错让用户走「获取密钥」。
        let mut blank =
            settings_config_for(&AppType::Codex, "", "n", "https://x.example/v1", "m").unwrap();
        assert_eq!(extract_api_key(&blank, &AppType::Codex), None);
        blank["auth"] = serde_json::json!({});
        assert_eq!(extract_api_key(&blank, &AppType::Codex), None);
    }

    #[test]
    fn patch_api_key_supports_top_level_and_nested_additive_configs() {
        for app_type in [AppType::Hermes, AppType::OpenClaw, AppType::OpenCode] {
            let mut settings =
                settings_config_for(&app_type, "sk-old", "n", "https://x.example/v1", "m")
                    .unwrap_or_else(|| panic!("{app_type:?} 必须有形状"));

            assert!(patch_api_key(&mut settings, &app_type, "sk-new"));
            assert_eq!(
                extract_api_key(&settings, &app_type).as_deref(),
                Some("sk-new")
            );
        }

        let mut missing_options = serde_json::json!({ "models": {} });
        assert!(!patch_api_key(
            &mut missing_options,
            &AppType::OpenCode,
            "sk-new"
        ));
        assert!(missing_options.get("options").is_none());

        assert!(ensure_api_key(
            &mut missing_options,
            &AppType::OpenCode,
            "sk-new"
        ));
        assert_eq!(
            extract_api_key(&missing_options, &AppType::OpenCode).as_deref(),
            Some("sk-new")
        );
    }

    #[test]
    fn settings_config_shapes_match_upstream_for_claude_and_gemini() {
        // claude / gemini 有意与上游 `UniversalProvider::to_*_provider()` 一致 ——
        // 上游加第 9 个 CLI 时我们照它抄，这条钉住「现在是抄来的」这个事实。
        let claude =
            settings_config_for(&AppType::Claude, "sk-c", "n", "https://a.example", "m").unwrap();
        assert_eq!(claude["env"]["ANTHROPIC_BASE_URL"], "https://a.example");
        assert_eq!(claude["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-c");
        // 三个默认模型也照上游给：不给的话 Claude Code 会按各自默认名请求，
        // 而中转站通常只认一个模型名。
        assert_eq!(claude["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "m");

        let gemini =
            settings_config_for(&AppType::Gemini, "sk-g", "n", "https://g.example", "m").unwrap();
        assert_eq!(gemini["env"]["GEMINI_API_KEY"], "sk-g");

        // codex **不能**照上游抄：上游那份写 requires_openai_auth = true，
        // 而我们实测那会让 codex 去打 chatgpt.com（见 codex_config_toml 的文档）。
        let codex =
            settings_config_for(&AppType::Codex, "sk-x", "n", "https://x.example/v1", "m").unwrap();
        assert!(
            !codex["config"]
                .as_str()
                .unwrap()
                .contains("requires_openai_auth"),
            "codex 那份不能照上游抄 —— 那个字段会让它去打 chatgpt.com 拿 403"
        );

        // 改用上游 deeplink 那套之后，**8 个 CLI 全都有形状了** —— 这是复用带来的：
        // `build_provider_from_request` 覆盖 claude/claudeDesktop/codex/gemini/
        // grokbuild/opencode/openclaw/hermes。所以那道「这个 CLI 接了没有」的闸
        // （`do_provision` 里）现在实际上不会拦下任何 CLI。
        //
        // 留着 Option 与那道闸**不是多余**：它让「上游哪天新增一个 app_type
        // 而我们还没验证过它」这件事有地方表达，而不是静默生成一份没验证过的配置。
        for app_type in AppType::all() {
            // Pi（上游 3.19.x 新增）还没接入中转站链路：在验证过它的配置形状前，
            // settings_config_for 对它返回 None —— do_provision 的闸会明确拦截，
            // 而不是静默生成一份没验证过的配置。这正是上面注释说的那个表达位。
            if matches!(app_type, AppType::Pi) {
                assert!(
                    settings_config_for(&app_type, "k", "n", "https://x.example", "m").is_none(),
                    "Pi 还没验证过中转站配置形状，应该显式返回 None"
                );
                continue;
            }
            assert!(
                settings_config_for(&app_type, "k", "n", "https://x.example", "m").is_some(),
                "{app_type:?} 没有配置形状 —— 上游 build_provider_from_request 该覆盖它"
            );
        }
    }

    #[test]
    fn extract_api_key_is_the_only_way_to_read_sk_across_clis() {
        // 这条钉住一个**真踩过的坑**：`relay_list_tier_rates` 原本硬编码
        // `settings_config.auth.OPENAI_API_KEY` 去抠 sk —— 那是 codex 的位置，
        // claude/gemini 的 sk 不在那儿 ⇒ 那两个平台**永远查不到倍率**，
        // 而且是静默的（filter_map 直接跳过，用户只看到「倍率未知」）。
        //
        // 所以：任何要读 sk 的地方都必须走 extract_api_key。
        let codex_path = |sc: &serde_json::Value| {
            sc.get("auth")
                .and_then(|a| a.get("OPENAI_API_KEY"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };

        for app_type in [AppType::Claude, AppType::Gemini] {
            let sc = settings_config_for(&app_type, "sk-real", "n", "https://x.example", "m")
                .unwrap_or_else(|| panic!("{app_type:?} 必须有形状"));

            // 硬编码 codex 路径读不到 —— 这正是原来那个 bug。
            assert_eq!(
                codex_path(&sc),
                None,
                "{app_type:?} 的 sk 不在 auth.OPENAI_API_KEY —— 硬编码那条路径会静默失败"
            );
            // 走 extract_api_key 就读得到。
            assert_eq!(extract_api_key(&sc, &app_type).as_deref(), Some("sk-real"));
        }
    }

    #[test]
    fn display_name_falls_back_to_group_when_site_name_is_blank() {
        assert_eq!(
            provider_display_name("Example Relay", "Pro"),
            "Example Relay · Pro"
        );
        assert_eq!(provider_display_name("", "Pro"), "Pro");
    }
}
