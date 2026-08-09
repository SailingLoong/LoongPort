//! 供应商连通性检查服务（reachability）
//!
//! 仅探测供应商 `base_url` 是否可达，**不发送真实大模型请求**：
//! - 收到任意 HTTP 响应（200/4xx/5xx）即判定"可达"（端口通、网关存活）；
//! - 仅 DNS / 连接被拒 / TLS / 超时等网络级错误判定"不可达"；
//! - 延迟 = 收到响应头的耗时（TTFB，真实往返）。
//!
//! ## 设计取舍：可达 ≠ 配置正确
//!
//! 本检查刻意不验证鉴权或模型，因此不会被第三方供应商的鉴权拦截 / 模型校验
//! 误判为"不可用"。代价是它无法告诉你鉴权对不对、模型存不存在。
//!
//! ## 与故障转移的关系（重要不变量）
//!
//! 连通性检查 **绝不** 触碰故障转移熔断器：一个返回 403/401 的供应商在本检查里
//! 算"可达"，但它对真实流量是坏的。熔断器只由 `proxy/forwarder.rs` 转发真实流量
//! 的成败驱动（被动）。两者职责分离——可达性回答"能不能到"，真实流量回答"能不能用"。

use reqwest::header::HeaderValue;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::Provider;
use crate::proxy::providers::{get_adapter, ClaudeAdapter, ProviderAdapter};

/// 健康状态枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Operational,
    Degraded,
    Failed,
}

/// `/models` 探测的结构化结论。
///
/// 该值会序列化进既有的 `stream_check_logs.model_used` TEXT 列；这样不需要迁移数据库，
/// 前端仍能按当前语言渲染文案。旧日志里的纯文本值由前端作为 legacy 值回退显示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ModelProbeVerdict {
    KeyExpired { status: u16 },
    Forbidden { status: u16 },
    NoModels,
    ImageOnly { models: Vec<String> },
    Models { total: usize, head: Vec<String> },
}

impl ModelProbeVerdict {
    fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// 连通性检查配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCheckConfig {
    /// 单次探测超时（秒）
    pub timeout_secs: u64,
    /// 超时类失败的最大重试次数
    pub max_retries: u32,
    /// 降级阈值（毫秒）：可达但 TTFB 超过该值判定为"较慢"
    pub degraded_threshold_ms: u64,
}

impl Default for StreamCheckConfig {
    fn default() -> Self {
        // 可达性探测打的是 base_url 的小请求（仅读响应头），不等待模型生成，故超时远小于
        // 旧的真实请求检查（45s → 8s）；降级阈值沿用旧尺度 6000ms——探测 TTFB 一般远低于
        // 此，仅在确实很慢时才标"较慢"，避免把 1 秒多的正常延迟误判为降级。
        Self {
            timeout_secs: 8,
            max_retries: 1,
            degraded_threshold_ms: 6000,
        }
    }
}

/// 连通性检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCheckResult {
    pub status: HealthStatus,
    pub success: bool,
    pub message: String,
    pub response_time_ms: Option<u64>,
    pub http_status: Option<u16>,
    /// 兼容 `stream_check_logs` 表结构的探测结论 JSON；未探测到时为空串。
    pub model_used: String,
    pub tested_at: i64,
    pub retry_count: u32,
    /// 细粒度错误分类；连通性检查不再细分，恒为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
}

/// 连通性检查服务
pub struct StreamCheckService;

impl StreamCheckService {
    /// 执行连通性检查（仅对超时类失败重试）。
    ///
    /// `base_url_override`：用于 Copilot 等需要从 OAuth 管理器动态解析端点的供应商，
    /// 由命令层预先解析后传入；其余供应商传 `None`，由本服务从 `settings_config` 提取。
    pub async fn check_with_retry(
        app_type: &AppType,
        provider: &Provider,
        config: &StreamCheckConfig,
        base_url_override: Option<String>,
    ) -> Result<StreamCheckResult, AppError> {
        let mut last_result: Option<StreamCheckResult> = None;
        for attempt in 0..=config.max_retries {
            let start = Instant::now();
            let result =
                Self::check_once(app_type, provider, config, base_url_override.clone(), start)
                    .await?;

            if result.success {
                return Ok(StreamCheckResult {
                    retry_count: attempt,
                    ..result
                });
            }

            // 仅超时 / abort 类网络抖动值得重试；连接被拒、DNS 失败等立即返回。
            if Self::should_retry(&result.message) && attempt < config.max_retries {
                last_result = Some(result);
                continue;
            }
            return Ok(StreamCheckResult {
                retry_count: attempt,
                ..result
            });
        }

        Ok(last_result.unwrap_or_else(|| StreamCheckResult {
            status: HealthStatus::Failed,
            success: false,
            message: "Check failed".to_string(),
            response_time_ms: None,
            http_status: None,
            model_used: String::new(),
            tested_at: chrono::Utc::now().timestamp(),
            retry_count: config.max_retries,
            error_category: None,
        }))
    }

    /// 单次连通性探测。
    async fn check_once(
        app_type: &AppType,
        provider: &Provider,
        config: &StreamCheckConfig,
        base_url_override: Option<String>,
        start: Instant,
    ) -> Result<StreamCheckResult, AppError> {
        let base_url = match base_url_override {
            Some(b) => b,
            None => Self::resolve_base_url(app_type, provider)?,
        };

        let client = crate::proxy::http_client::get();
        let timeout = std::time::Duration::from_secs(config.timeout_secs);
        let ua = Self::custom_user_agent(provider);

        let result = Self::probe_reachability(&client, &base_url, timeout, ua.clone()).await;
        let response_time = start.elapsed().as_millis() as u64;
        let mut checked = Self::build_result(result, response_time, config.degraded_threshold_ms);

        // 可达之后再问一句「这个档位到底能调什么」—— 见 `probe_models`。
        // 只在**托管档位**上做：判据要 sk，而那套形状只有托管项保证有；
        // 用户手工建的 provider 密钥可能在任意位置。
        if checked.success && crate::relay::is_managed(&provider.id) {
            if let Some(verdict) =
                Self::probe_models(&client, app_type, provider, &base_url, timeout, ua).await
            {
                checked.model_used = verdict.encode();
            }
        }
        Ok(checked)
    }

    /// 问这个档位**真正能调哪些模型**（`GET {base_url}/models`），零成本。
    ///
    /// # 为什么可达性探测不够
    ///
    /// [`probe_reachability`] 只答「端口通不通」—— 它对任何 HTTP 响应都算成功。而档位
    /// 真正的失效方式往往在**那之后**：
    ///
    /// | 失效方式 | 可达性探测 | 本探测 |
    /// |---|---|---|
    /// | 域名挂了 / 端口不通 | ✅ 抓得到 | ✅ |
    /// | sk 被删 / 过期（401） | ❌ 说"可达" | ✅ |
    /// | 分组没挂任何模型（调用必失败） | ❌ 说"可达" | ✅ |
    /// | 分组只挂了生图模型却当对话档位用 | ❌ 说"可达" | ✅ |
    ///
    /// 最后那条是实测踩到的：鑫旺 Neko API 的两个生图分组在可达性探测里全是"正常"，
    /// 而拿它们对话稳定 502。**用户是在账单或报错里才知道的** —— 那正是这个探测要
    /// 提前告诉他的事。
    ///
    /// # 为什么零成本
    ///
    /// `/v1/models` 是**列表接口，不计费**（不产生 token、不触发调度）。所以它可以随手点、
    /// 可以对每个档位都点，与「真发一次推理去试」有本质区别 —— 后者要花钱，因而不可能
    /// 做成一个用户随时能按的按钮。
    ///
    /// # 返回值序列化后放进 `model_used`（一个原本恒空的字段）
    ///
    /// `StreamCheckResult::model_used` 是 `stream_check_logs` 表里的既有列，改成真实检查
    /// 之后它一直是空串。填上结构化 JSON ⇒ 前端与历史日志**不用改结构**就能看到这条信息，
    /// 而这正是那个字段当初的用意。旧行若仍是纯文本，由调用方按 legacy 值处理。
    ///
    /// 返回 `None` = 问不出来（站点没这个端点 / 网络抖动）。**那时不改判定** ——
    /// 探测不到不等于档位坏了，把「我不知道」报成「不可用」比不报更糟。
    async fn probe_models(
        client: &Client,
        app_type: &AppType,
        provider: &Provider,
        base_url: &str,
        timeout: std::time::Duration,
        custom_ua: Option<HeaderValue>,
    ) -> Option<ModelProbeVerdict> {
        // 密钥位置按 CLI 分派，复用那一处定义（硬编码 codex 的位置会让 claude 档位
        // 永远探不出来，而且是静默的）。
        let api_key =
            crate::relay::provision::extract_api_key(&provider.settings_config, app_type)?;

        let url = format!("{}/models", base_url.trim_end_matches('/'));
        let mut req = client
            .get(&url)
            .bearer_auth(&api_key)
            .timeout(timeout)
            .header("accept", "application/json");
        if let Some(ua) = custom_ua {
            req = req.header("user-agent", ua);
        }

        let resp = req.send().await.ok()?;
        let status = resp.status();
        if !status.is_success() {
            // 401 / 403 是**确定的坏消息**（密钥失效 / 无权限），值得报出来 ——
            // 它正是可达性探测看不见的那一类。其余非 2xx 说明这个站没有这个端点，
            // 那不是档位的问题，按「问不出来」处理。
            return match status.as_u16() {
                401 => Some(ModelProbeVerdict::KeyExpired { status: 401 }),
                403 => Some(ModelProbeVerdict::Forbidden { status: 403 }),
                _ => None,
            };
        }

        let body = resp.text().await.ok()?;
        let models: Vec<String> = serde_json::from_str::<serde_json::Value>(&body)
            .ok()?
            .get("data")?
            .as_array()?
            .iter()
            .filter_map(|m| m.get("id")?.as_str().map(str::to_string))
            .filter(|id| !id.is_empty())
            .collect();

        if models.is_empty() {
            return Some(ModelProbeVerdict::NoModels);
        }

        // 只挂生图模型 ⇒ 当对话档位用必定失败。这句话要说得让用户能照做。
        let all_image = models
            .iter()
            .all(|m| crate::relay::provision::is_image_model(m));
        if all_image {
            return Some(ModelProbeVerdict::ImageOnly { models });
        }

        // 正常情况：报个数 + 头几个名字。全列出来会把 toast 撑爆（实测有 13 个的分组）。
        const SHOWN: usize = 3;
        let mut head: Vec<String> = models.iter().take(SHOWN).cloned().collect();
        if models.len() > SHOWN {
            head.push("…".to_string());
        }
        Some(ModelProbeVerdict::Models {
            total: models.len(),
            head,
        })
    }

    /// 解析供应商 `base_url`。
    ///
    /// 连通性探测只需打到 base（origin 或用户配置的 base 路径）即可——任何 HTTP
    /// 响应都证明端口可达，因此无需像旧的真实请求检查那样解析具体 API 路径
    /// （`/v1/messages` vs `/chat/completions` vs `:streamGenerateContent`）。
    ///
    /// 官方供应商（`category == "official"`）base_url 故意留空（走客户端默认/OAuth 端点），
    /// 没有 cc-switch 能可靠探测的目标——这类供应商的连通检测按钮在前端已隐藏
    /// （见 `ProviderCard.tsx`），故此处对其提取失败直接报错即可，不做官方端点回退。
    fn resolve_base_url(app_type: &AppType, provider: &Provider) -> Result<String, AppError> {
        if provider.category.as_deref() == Some("official") {
            return Err(AppError::Message(
                "Official providers do not expose a reachability-check target".to_string(),
            ));
        }

        match app_type {
            // 累加模式应用的 settings_config 结构与 Claude/Codex/Gemini 不同，
            // 不走 adapter，直接按各自约定提取 base_url。
            AppType::OpenCode => {
                let npm = Self::extract_opencode_npm(provider);
                Self::resolve_opencode_base_url(provider, npm.as_deref())
            }
            AppType::OpenClaw => Self::extract_openclaw_base_url(provider),
            AppType::Hermes => Self::extract_hermes_base_url(provider),
            AppType::ClaudeDesktop => ClaudeAdapter::new()
                .extract_base_url(provider)
                .map_err(|e| AppError::Message(format!("Failed to extract base_url: {e}"))),
            _ => get_adapter(app_type)
                .extract_base_url(provider)
                .map_err(|e| AppError::Message(format!("Failed to extract base_url: {e}"))),
        }
    }

    /// 轻量可达性探测：GET `base_url`，收到任意 HTTP 响应即可达。
    ///
    /// - `send()` 在收到响应头时即返回，故计时天然是 TTFB；不读 body。
    /// - reqwest 对任何 HTTP 状态码都返回 `Ok`，只有网络级错误进 `Err`——
    ///   这正是"任何响应都算可达、只有连不上才算失败"的语义。
    async fn probe_reachability(
        client: &Client,
        base_url: &str,
        timeout: std::time::Duration,
        custom_ua: Option<HeaderValue>,
    ) -> Result<u16, AppError> {
        let url = base_url.trim();
        if url.is_empty() {
            return Err(AppError::Message("base_url 为空".to_string()));
        }

        let mut req = client
            .get(url)
            .timeout(timeout)
            .header("accept", "*/*")
            .header("accept-encoding", "identity");
        // 复用供应商自定义 UA（部分网关按 UA 白名单放行），与转发路径口径一致。
        if let Some(ua) = custom_ua {
            req = req.header("user-agent", ua);
        }

        match req.send().await {
            Ok(resp) => Ok(resp.status().as_u16()),
            Err(e) => Err(Self::map_request_error(e)),
        }
    }

    /// 将探测原始结果包装成 `StreamCheckResult`。
    fn build_result(
        result: Result<u16, AppError>,
        response_time: u64,
        degraded_threshold_ms: u64,
    ) -> StreamCheckResult {
        let tested_at = chrono::Utc::now().timestamp();
        match result {
            Ok(status) => StreamCheckResult {
                status: Self::determine_status(response_time, degraded_threshold_ms),
                success: true,
                message: "Reachable".to_string(),
                response_time_ms: Some(response_time),
                http_status: Some(status),
                model_used: String::new(),
                tested_at,
                retry_count: 0,
                error_category: None,
            },
            Err(e) => StreamCheckResult {
                status: HealthStatus::Failed,
                success: false,
                message: e.to_string(),
                response_time_ms: Some(response_time),
                http_status: None,
                model_used: String::new(),
                tested_at,
                retry_count: 0,
                error_category: None,
            },
        }
    }

    fn determine_status(latency_ms: u64, threshold: u64) -> HealthStatus {
        if latency_ms <= threshold {
            HealthStatus::Operational
        } else {
            HealthStatus::Degraded
        }
    }

    fn should_retry(msg: &str) -> bool {
        let lower = msg.to_lowercase();
        lower.contains("timeout") || lower.contains("abort") || lower.contains("timed out")
    }

    fn map_request_error(e: reqwest::Error) -> AppError {
        if e.is_timeout() {
            AppError::Message("Request timeout".to_string())
        } else if e.is_connect() {
            AppError::Message(format!("Connection failed: {e}"))
        } else {
            AppError::Message(e.to_string())
        }
    }

    /// Provider 级自定义 User-Agent（`meta.customUserAgent`），与转发路径共用单一口径：
    /// trim、空串视为未设置、非法值静默忽略（返回 `None`）。
    fn custom_user_agent(provider: &Provider) -> Option<HeaderValue> {
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.custom_user_agent_header().ok().flatten())
    }

    // ===== 各应用 base_url 提取（settings_config 结构互不相同）=====

    /// OpenClaw: `{ baseUrl, apiKey, api, ... }`（camelCase）
    fn extract_openclaw_base_url(provider: &Provider) -> Result<String, AppError> {
        provider
            .settings_config
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                AppError::localized(
                    "openclaw_base_url_missing",
                    "OpenClaw 供应商缺少 baseUrl",
                    "OpenClaw provider is missing `baseUrl`",
                )
            })
    }

    /// Hermes: `{ base_url, api_key, api_mode }`（snake_case）
    fn extract_hermes_base_url(provider: &Provider) -> Result<String, AppError> {
        provider
            .settings_config
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                AppError::localized(
                    "hermes_base_url_missing",
                    "Hermes 供应商缺少 base_url",
                    "Hermes provider is missing `base_url`",
                )
            })
    }

    /// OpenCode: `{ npm, options: { baseURL, apiKey }, ... }`
    ///
    /// 用户未显式填 `options.baseURL` 时，按 `npm`（AI SDK 包）回退到包自带默认端点。
    /// `@ai-sdk/openai-compatible` 无默认端点，必须显式填。
    fn resolve_opencode_base_url(
        provider: &Provider,
        npm: Option<&str>,
    ) -> Result<String, AppError> {
        if let Some(explicit) = Self::extract_opencode_base_url(provider) {
            return Ok(explicit);
        }

        let fallback = match npm {
            Some("@ai-sdk/openai") => Some("https://api.openai.com/v1"),
            Some("@ai-sdk/anthropic") => Some("https://api.anthropic.com"),
            Some("@ai-sdk/google") => Some("https://generativelanguage.googleapis.com"),
            _ => None,
        };

        fallback.map(|s| s.to_string()).ok_or_else(|| {
            AppError::localized(
                "opencode_base_url_missing",
                "OpenCode 供应商缺少 options.baseURL，且当前 SDK 包没有默认端点",
                "OpenCode provider is missing `options.baseURL` and the SDK package has no default endpoint",
            )
        })
    }

    fn extract_opencode_base_url(provider: &Provider) -> Option<String> {
        provider
            .settings_config
            .get("options")
            .and_then(|v| v.get("baseURL"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn extract_opencode_npm(provider: &Provider) -> Option<String> {
        provider
            .settings_config
            .get("npm")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(settings_config: serde_json::Value) -> Provider {
        Provider::with_id(
            "test".to_string(),
            "Test".to_string(),
            settings_config,
            None,
        )
    }

    #[test]
    fn test_default_config_uses_reachability_friendly_values() {
        let config = StreamCheckConfig::default();
        assert_eq!(config.timeout_secs, 8);
        assert_eq!(config.max_retries, 1);
        // 降级阈值沿用旧尺度，避免把 1 秒多的正常延迟误判为"较慢"
        assert_eq!(config.degraded_threshold_ms, 6000);
    }

    #[test]
    fn model_probe_verdict_serializes_for_existing_log_column() {
        let verdict = ModelProbeVerdict::Models {
            total: 4,
            head: vec!["alpha".to_string(), "beta".to_string(), "…".to_string()],
        };

        assert_eq!(
            verdict.encode(),
            r#"{"kind":"models","total":4,"head":["alpha","beta","…"]}"#
        );
        assert_eq!(
            serde_json::from_str::<ModelProbeVerdict>(&verdict.encode()).unwrap(),
            verdict
        );
    }

    #[test]
    fn test_determine_status() {
        assert_eq!(
            StreamCheckService::determine_status(1000, 1500),
            HealthStatus::Operational
        );
        assert_eq!(
            StreamCheckService::determine_status(1500, 1500),
            HealthStatus::Operational
        );
        assert_eq!(
            StreamCheckService::determine_status(1501, 1500),
            HealthStatus::Degraded
        );
    }

    #[test]
    fn test_should_retry_only_on_timeout_like_errors() {
        assert!(StreamCheckService::should_retry("Request timeout"));
        assert!(StreamCheckService::should_retry("request timed out"));
        assert!(StreamCheckService::should_retry("connection abort"));
        // 连接被拒 / DNS 失败不重试
        assert!(!StreamCheckService::should_retry(
            "Connection failed: dns error"
        ));
        assert!(!StreamCheckService::should_retry("Reachable"));
    }

    #[test]
    fn test_build_result_any_http_status_is_reachable() {
        // 任何 HTTP 状态码都算可达（success=true）
        for status in [200u16, 401, 403, 404, 429, 500, 503] {
            let r = StreamCheckService::build_result(Ok(status), 100, 1500);
            assert!(r.success, "status {status} should be reachable");
            assert_eq!(r.status, HealthStatus::Operational);
            assert_eq!(r.http_status, Some(status));
            assert!(r.model_used.is_empty());
            assert!(r.error_category.is_none());
        }
    }

    #[test]
    fn test_build_result_network_error_is_unreachable() {
        let r = StreamCheckService::build_result(
            Err(AppError::Message("Connection failed: refused".to_string())),
            5,
            1500,
        );
        assert!(!r.success);
        assert_eq!(r.status, HealthStatus::Failed);
        assert!(r.http_status.is_none());
    }

    #[test]
    fn test_build_result_slow_response_is_degraded() {
        let r = StreamCheckService::build_result(Ok(200), 3000, 1500);
        assert!(r.success);
        assert_eq!(r.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_resolve_opencode_base_url_explicit_wins() {
        let p = make_provider(serde_json::json!({
            "npm": "@ai-sdk/openai",
            "options": { "baseURL": "https://proxy.local/v1", "apiKey": "k" },
            "models": {},
        }));
        let resolved =
            StreamCheckService::resolve_opencode_base_url(&p, Some("@ai-sdk/openai")).unwrap();
        assert_eq!(resolved, "https://proxy.local/v1");
    }

    #[test]
    fn test_resolve_opencode_base_url_falls_back_for_known_npm() {
        let p = make_provider(serde_json::json!({
            "npm": "@ai-sdk/anthropic",
            "options": { "apiKey": "k" },
            "models": {},
        }));
        let resolved =
            StreamCheckService::resolve_opencode_base_url(&p, Some("@ai-sdk/anthropic")).unwrap();
        assert_eq!(resolved, "https://api.anthropic.com");
    }

    #[test]
    fn test_resolve_opencode_base_url_errors_for_openai_compatible_without_url() {
        let p = make_provider(serde_json::json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": { "apiKey": "k" },
            "models": {},
        }));
        let result =
            StreamCheckService::resolve_opencode_base_url(&p, Some("@ai-sdk/openai-compatible"));
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_openclaw_base_url_missing_errors() {
        let p = make_provider(serde_json::json!({ "apiKey": "k", "api": "openai-completions" }));
        assert!(StreamCheckService::extract_openclaw_base_url(&p).is_err());

        let p2 = make_provider(serde_json::json!({ "baseUrl": "https://api.deepseek.com/v1" }));
        assert_eq!(
            StreamCheckService::extract_openclaw_base_url(&p2).unwrap(),
            "https://api.deepseek.com/v1"
        );
    }

    #[test]
    fn test_resolve_base_url_uses_explicit_url_or_errors_when_missing() {
        // 有显式 base_url → 直接用
        let p = make_provider(
            serde_json::json!({ "env": { "ANTHROPIC_BASE_URL": "https://relay.example/v1" } }),
        );
        assert_eq!(
            StreamCheckService::resolve_base_url(&AppType::Claude, &p).unwrap(),
            "https://relay.example/v1"
        );

        // 缺 base_url（官方留空 / 用户忘填）→ 报错。官方供应商的检测按钮在前端已隐藏，
        // 不会走到这里；不做官方端点回退（避免给忘填地址的第三方误显绿灯）。
        let empty = make_provider(serde_json::json!({ "env": {} }));
        assert!(StreamCheckService::resolve_base_url(&AppType::Claude, &empty).is_err());

        let mut official = make_provider(serde_json::json!({ "auth": {}, "config": "" }));
        official.id = crate::database::CODEX_OFFICIAL_PROVIDER_ID.to_string();
        official.category = Some("official".to_string());
        assert!(StreamCheckService::resolve_base_url(&AppType::Codex, &official).is_err());
    }
}
