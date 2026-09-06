//! 站点自报声明的应用与一键回退（site_config）。
//! 更大的图景与约束见本目录 mod.rs 的总览。

use super::*;
use crate::relay::provision;
use crate::relay::site_config;

/// 应用站长自报调用配置（`relay/site_config.rs`）的摘要：哪些档位吃到了声明段。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteConfigAppliedTier {
    pub app_id: String,
    pub provider_id: String,
    pub display_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteConfigApplySummary {
    pub site_origin: String,
    pub declared_origin: String,
    pub applied: Vec<SiteConfigAppliedTier>,
}

/// 应用站长自报的调用配置：用户贴入站长给的 URL / JSON / base64，把站长维护的
/// 各平台默认配置同步到该站点的托管档位上。
///
/// 与自动路径（首次导入探测，见 `persist_provision_batch`）的语义差异：这是用户
/// **显式动作**，允许覆盖用户编辑过的配置——用户明确要站长的这套；自动路径永远
/// 不碰用户编辑。双重同源校验：手输 URL 先拦（不发往第三方域），内容里声明的
/// `site_origin` 再拦（防内容冒名）。只对**该站点的托管档位**生效（`website_url`
/// 等值匹配，宁漏配不误配）。
#[tauri::command]
pub async fn relay_apply_site_config(
    app_handle: tauri::AppHandle,
    relay_id: i64,
    input: String,
) -> Result<SiteConfigApplySummary, AppError> {
    let op = usable_relay(&app_handle, relay_id).await?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            "请粘贴站长提供的配置链接或内容".into(),
        ));
    }
    let declared = if trimmed.starts_with("https://") {
        site_config::validate_same_origin(trimmed, &op.site_origin)?;
        site_config::fetch_declaration_from_url(trimmed).await?
    } else {
        site_config::parse_site_config(trimmed)?
    };
    site_config::validate_same_origin(&declared.site_origin, &op.site_origin)?;

    let state = app_handle.state::<crate::store::AppState>();
    let mut applied = Vec::new();
    // 声明里出现的每个平台 → 对应 app 的该站点托管档位逐个应用。
    // platform 键来自 parse_platform（schema 合法键集），app_type() 拿 Mapped 目标。
    let platforms: Vec<_> = declared
        .platforms
        .keys()
        .filter_map(|key| platform_map::parse_platform(key))
        .collect();
    for platform in platforms {
        let Some(app_type) = platform.app_type() else {
            continue;
        };
        let Some(segment) = declared.segment_for(platform) else {
            continue;
        };
        let providers = ProviderService::list(&state, app_type.clone())?;
        for mut provider in providers.into_values() {
            if !is_managed(&provider) {
                continue;
            }
            if provider.website_url.as_deref() != Some(op.site_origin.as_str()) {
                continue;
            }
            if !site_config::apply_segment_to_app(
                &app_type,
                segment,
                &mut provider.settings_config,
            )? {
                continue;
            }
            let meta = provider.meta.get_or_insert_with(Default::default);
            meta.site_declared_origin = Some(declared.site_origin.clone());
            state
                .db
                .save_provider(app_type.as_str(), &provider)
                .map_err(|e| AppError::Config(format!("保存档位 {} 失败: {e}", provider.name)))?;
            applied.push(SiteConfigAppliedTier {
                app_id: app_type.as_str().to_string(),
                provider_id: provider.id.clone(),
                display_name: provider.name.clone(),
            });
        }
    }
    if applied.is_empty() {
        return Err(AppError::Config(
            "该站点下没有可应用声明的托管档位——先登录或获取密钥建立档位".into(),
        ));
    }
    Ok(SiteConfigApplySummary {
        site_origin: op.site_origin,
        declared_origin: declared.site_origin,
        applied,
    })
}

/// 从现有 settings_config 提取重建默认所需的 (api_key, base_url, model)。
///
/// 「恢复内置默认」要把档位重建回 `settings_config_for` 的形状，但 sk 与端点
/// 必须原样保留（它们来自用户登录，重建不是换钥匙）。四个 app 的读取位置与
/// `persist_provision_batch` 的写入位置一一对应；读不出任何一项就返回 `None`
///（调用方跳过该档位——重建出一把没有钥匙的默认配置比不重建更糟）。
pub(crate) fn rebuild_inputs_from_settings(
    app_type: &AppType,
    settings: &serde_json::Value,
) -> Option<(String, String, String)> {
    let env = settings.get("env").and_then(|v| v.as_object());
    match app_type {
        AppType::Claude | AppType::ClaudeDesktop => {
            let env = env?;
            let api_key = env
                .get("ANTHROPIC_AUTH_TOKEN")
                .or_else(|| env.get("ANTHROPIC_API_KEY"))?
                .as_str()?;
            let base_url = env.get("ANTHROPIC_BASE_URL")?.as_str()?;
            let model = env.get("ANTHROPIC_MODEL").and_then(|v| v.as_str())?;
            Some((api_key.to_string(), base_url.to_string(), model.to_string()))
        }
        AppType::Gemini => {
            let env = env?;
            let api_key = env.get("GEMINI_API_KEY")?.as_str()?;
            let base_url = env.get("GOOGLE_GEMINI_BASE_URL")?.as_str()?;
            let model = env.get("GEMINI_MODEL").and_then(|v| v.as_str())?;
            Some((api_key.to_string(), base_url.to_string(), model.to_string()))
        }
        AppType::Codex | AppType::CodexImage => {
            let api_key = settings
                .get("auth")?
                .get("OPENAI_API_KEY")?
                .as_str()?
                .to_string();
            let toml_text = settings.get("config")?.as_str()?;
            let toml_value: toml::Value = toml::from_str(toml_text).ok()?;
            let base_url = toml_value
                .get("model_providers")?
                .get("custom")?
                .get("base_url")?
                .as_str()?
                .to_string();
            let model = toml_value.get("model")?.as_str()?.to_string();
            Some((api_key, base_url, model))
        }
        AppType::GrokBuild => {
            let inner: serde_json::Value =
                serde_json::from_str(settings.get("config")?.as_str()?).ok()?;
            let api_key = inner.get("apiKey")?.as_str()?.to_string();
            let base_url = inner.get("baseUrl")?.as_str()?.to_string();
            let model = provision::selected_model(app_type, settings).unwrap_or_default();
            Some((api_key, base_url, model))
        }
        _ => None,
    }
}

/// 恢复内置默认：把该站点的全部托管档位重建回 `settings_config_for` 的形状
/// （sk / 端点 / 当前模型选择原样保留），并清掉「站点推荐配置」标注。
///
/// 「一键回退」的数据面（spec：站点声明的值可发现、可回退）。与
/// [`relay_apply_site_config`] 对称：一个把站长声明合进来，一个退回去。
#[tauri::command]
pub async fn relay_reset_site_config(
    app_handle: tauri::AppHandle,
    relay_id: i64,
) -> Result<SiteConfigApplySummary, AppError> {
    let op = usable_relay(&app_handle, relay_id).await?;
    let state = app_handle.state::<crate::store::AppState>();

    let mut applied = Vec::new();
    // 遍历名单从 [`provision::model_catalog_apps`] 派生（不再是手写字符串数组 ——
    // 那份曾经漏过生图档、和别处的平台名单各自漂移）。生图档位本来就从
    // `rebuild_inputs_from_settings` 读不出重建要素，不在此列。
    for app_type in provision::model_catalog_apps() {
        let providers = ProviderService::list(&state, app_type.clone())?;
        for mut provider in providers.into_values() {
            if !is_managed(&provider) {
                continue;
            }
            if provider.website_url.as_deref() != Some(op.site_origin.as_str()) {
                continue;
            }
            let Some((api_key, base_url, model)) =
                rebuild_inputs_from_settings(app_type, &provider.settings_config)
            else {
                log::warn!(
                    "{} 的配置读不出重建要素（sk/端点/模型），跳过恢复默认",
                    provider.name
                );
                continue;
            };
            let Some(defaults) = provision::settings_config_for(
                app_type,
                &api_key,
                &provider.name,
                &base_url,
                &model,
            ) else {
                continue;
            };
            provider.settings_config = defaults;
            if let Some(meta) = provider.meta.as_mut() {
                meta.site_declared_origin = None;
            }
            state
                .db
                .save_provider(app_type.as_str(), &provider)
                .map_err(|e| AppError::Config(format!("保存档位 {} 失败: {e}", provider.name)))?;
            applied.push(SiteConfigAppliedTier {
                app_id: app_type.as_str().to_string(),
                provider_id: provider.id.clone(),
                display_name: provider.name.clone(),
            });
        }
    }
    if applied.is_empty() {
        return Err(AppError::Config("该站点下没有可恢复默认的托管档位".into()));
    }
    Ok(SiteConfigApplySummary {
        site_origin: op.site_origin,
        declared_origin: String::new(),
        applied,
    })
}
