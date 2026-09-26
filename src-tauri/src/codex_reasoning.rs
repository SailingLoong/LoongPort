//! Pure Codex reasoning capabilities shared by catalog projection and request adapters.
//! Callers supply model/transport facts; this module never performs I/O or reads global state.
use crate::provider::CodexChatReasoningConfig;
use serde_json::Value;

pub(crate) const EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

pub(crate) fn canonical_levels(levels: &[String]) -> Vec<String> {
    EFFORTS
        .iter()
        .filter(|effort| levels.iter().any(|candidate| candidate == **effort))
        .map(|effort| (*effort).to_string())
        .collect()
}

pub(crate) fn effort_rank(effort: &str) -> Option<usize> {
    EFFORTS.iter().position(|candidate| *candidate == effort)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReasoningCapabilities {
    pub levels: Vec<String>,
    /// Only an explicitly declared or authoritative default is usable for request correction.
    pub default_level: Option<String>,
}

impl ReasoningCapabilities {
    /// A model switch can reuse a supported effort or its authoritative default.
    pub(crate) fn selected_effort(&self, model: &str, requested: &str) -> Result<String, String> {
        if self.levels.iter().any(|level| level == requested) {
            return Ok(requested.to_string());
        }
        self.default_level.clone().ok_or_else(|| {
            format!(
                "Model {model} does not support reasoning effort {requested}; choose one of: {}",
                self.levels.join(", ")
            )
        })
    }
}

pub(crate) fn model_row<'a>(settings: &'a Value, model: &str) -> Option<&'a Value> {
    settings
        .get("modelCatalog")?
        .get("models")?
        .as_array()?
        .iter()
        .find(|row| {
            row.get("model")
                .and_then(Value::as_str)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(model))
        })
}

pub(crate) fn declared_capabilities(row: &Value) -> Option<ReasoningCapabilities> {
    let levels = row
        .get("reasoningLevels")
        .or_else(|| row.get("reasoning_levels"))?
        .as_array()?;
    let levels = canonical_levels(
        &levels
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>(),
    );
    if levels.is_empty() {
        return None;
    }
    let default_level = row
        .get("defaultReasoningLevel")
        .or_else(|| row.get("default_reasoning_level"))
        .and_then(Value::as_str)
        .filter(|value| levels.iter().any(|level| level == value))
        .map(str::to_string);
    Some(ReasoningCapabilities {
        levels,
        default_level,
    })
}

pub(crate) fn native_capabilities(entry: &Value) -> Option<ReasoningCapabilities> {
    let levels = entry.get("supported_reasoning_levels")?.as_array()?;
    let levels = canonical_levels(
        &levels
            .iter()
            .filter_map(|level| level.get("effort").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>(),
    );
    if levels.is_empty() {
        return None;
    }
    let default_level = entry
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .filter(|value| levels.iter().any(|level| level == value))
        .map(str::to_string);
    Some(ReasoningCapabilities {
        levels,
        default_level,
    })
}

pub(crate) fn curated_capabilities(model: &str) -> Option<ReasoningCapabilities> {
    let catalog: Value = serde_json::from_str(include_str!(
        "resources/codex_curated_reasoning_levels.json"
    ))
    .expect("bundled reasoning capabilities must be valid JSON");
    let rows = catalog.get("models")?.as_array()?;
    rows.iter()
        .find(|row| {
            row.get("model")
                .and_then(Value::as_str)
                .is_some_and(|slug| slug.eq_ignore_ascii_case(model))
        })
        .or_else(|| {
            rows.iter().find(|row| {
                row.get("model")
                    .and_then(Value::as_str)
                    .is_some_and(|slug| {
                        slug.rsplit('/')
                            .next()
                            .is_some_and(|slug| slug.eq_ignore_ascii_case(model))
                    })
            })
        })
        .and_then(declared_capabilities)
}

/// Explicit model facts win over official/curated facts. Transport restrictions still win:
/// a model capability does not authorize parameters its gateway explicitly does not support.
pub(crate) fn resolve(
    model: &str,
    declared: Option<ReasoningCapabilities>,
    official: Option<ReasoningCapabilities>,
    transport: Option<&CodexChatReasoningConfig>,
) -> Option<ReasoningCapabilities> {
    let normalize = |mut capabilities: ReasoningCapabilities| {
        capabilities.levels = canonical_levels(&capabilities.levels);
        capabilities.default_level = capabilities
            .default_level
            .take()
            .filter(|value| capabilities.levels.contains(value));
        (!capabilities.levels.is_empty()).then_some(capabilities)
    };
    let mut capabilities = declared
        .and_then(normalize)
        .or_else(|| official.and_then(normalize))
        .or_else(|| curated_capabilities(model));
    let Some(transport) = transport else {
        return capabilities;
    };
    if transport.supports_effort == Some(false) {
        let can_toggle = matches!(
            transport
                .thinking_param
                .as_deref()
                .unwrap_or("thinking")
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "thinking" | "enable_thinking" | "reasoning_split"
        );
        let levels = match (transport.supports_thinking == Some(true), can_toggle) {
            (true, true) => vec!["none".into(), "high".into()],
            (true, false) => vec!["high".into()],
            (false, _) => vec!["none".into()],
        };
        return Some(ReasoningCapabilities {
            default_level: Some(
                if transport.supports_thinking == Some(true) {
                    "high"
                } else {
                    "none"
                }
                .into(),
            ),
            levels,
        });
    }
    let allowed: Option<&[&str]> = match transport.effort_value_mode.as_deref() {
        Some("deepseek") => Some(&["none", "high", "max"]),
        Some("low_high") => Some(&["none", "low", "high"]),
        Some("openrouter") => Some(&["none", "minimal", "low", "medium", "high", "xhigh"]),
        _ => None,
    };
    if let Some(allowed) = allowed {
        if let Some(capabilities) = capabilities.as_mut() {
            capabilities
                .levels
                .retain(|level| allowed.contains(&level.as_str()));
            capabilities.default_level = capabilities
                .default_level
                .take()
                .filter(|value| capabilities.levels.contains(value));
        } else {
            capabilities = Some(ReasoningCapabilities {
                levels: allowed.iter().map(|level| (*level).into()).collect(),
                default_level: None,
            });
        }
    }
    capabilities
}

/// Fix saved contradictory enabled+none states without erasing disabled mappings.
pub(crate) fn normalize_chat_transport(
    mut config: CodexChatReasoningConfig,
) -> CodexChatReasoningConfig {
    if config.supports_effort == Some(true) {
        if config.supports_thinking.is_none() {
            config.supports_thinking = Some(true);
        }
        if config
            .effort_param
            .as_deref()
            .is_none_or(|param| param.trim().is_empty() || param == "none")
        {
            config.effort_param = Some("reasoning_effort".into());
        }
    }
    if config.supports_thinking == Some(true)
        && config.supports_effort != Some(true)
        && config
            .thinking_param
            .as_deref()
            .is_none_or(|param| param.trim().is_empty())
    {
        config.thinking_param = Some("thinking".into());
    }
    config
}

pub(crate) fn chat_transport(
    name: &str,
    base_url: &str,
    model: &str,
    explicit: Option<&CodexChatReasoningConfig>,
    settings: &Value,
    official: Option<ReasoningCapabilities>,
) -> Option<CodexChatReasoningConfig> {
    let declared = model_row(settings, model).and_then(declared_capabilities);
    let capabilities = resolve(model, declared.clone(), official.clone(), None);
    let mut config = explicit
        .cloned()
        .map(normalize_chat_transport)
        .or_else(|| infer_chat_transport(name, base_url, model))
        .or_else(|| {
            capabilities
                .as_ref()
                .map(|capabilities| CodexChatReasoningConfig {
                    supports_effort: Some(capabilities.levels.iter().any(|level| level != "none")),
                    supports_thinking: Some(false),
                    thinking_param: Some("none".into()),
                    effort_param: Some("reasoning_effort".into()),
                    effort_value_mode: Some("passthrough".into()),
                    ..Default::default()
                })
        })?;
    if config.supports_effort.is_none() && declared.is_some() {
        config.supports_effort = Some(true);
        config = normalize_chat_transport(config);
    }
    // Zen without an explicit per-model declaration must not invent an effort.
    // Other adapters can use authoritative model declarations without guessing aliases.
    config.effort_levels = if config.effort_value_mode.as_deref() == Some("zen") {
        declared.map(|capabilities| capabilities.levels)
    } else {
        resolve(model, declared, official, Some(&config)).map(|capabilities| capabilities.levels)
    };
    Some(config)
}

pub(crate) fn infer_chat_transport(
    name: &str,
    base_url: &str,
    model: &str,
) -> Option<CodexChatReasoningConfig> {
    let name = name.to_ascii_lowercase();
    let base_url = base_url.to_ascii_lowercase();
    let model = model.to_ascii_lowercase();
    // 平台优先：聚合 / 托管平台的 reasoning 接口由平台的推理框架决定，而非模型官方实现，
    // 因此先按平台标识（仅 name + base_url，不含 model 名）判定并覆盖模型规则。
    if let Some(config) = infer_aggregator_platform_config(&name, &base_url) {
        return Some(config);
    }

    let haystack = format!("{name} {base_url} {model}");

    if haystack.contains("deepseek") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("reasoning_effort".to_string()),
            effort_value_mode: Some("deepseek".to_string()),
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    // StepFun：官方 reasoning 指南与两站模型页（2026-08-15 盘点）——
    // step-3.5-flash-2603 支持 low/high 两档；step-3.7-flash 支持
    // low/medium/high 三档（官方默认 medium）；其余 step 模型（含无后缀
    // step-3.5-flash）不暴露 effort。2603 沿用 low_high 收敛映射；
    // 3.7-flash 必须 passthrough——套 low_high 会把 medium 塌成 high，
    // 造出 wire 上无差异的假档位。全系无思考开关（thinking_param 恒 none）。
    // 第二个 OR 分支覆盖「经中转/聚合跑该模型、但平台 name/base_url 不含 stepfun」的情况。
    if haystack.contains("stepfun") || haystack.contains("step-3.5-flash-2603") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(model.contains("2603") || model.contains("step-3.7-flash")),
            thinking_param: Some("none".to_string()),
            effort_param: Some("reasoning_effort".to_string()),
            effort_value_mode: Some(
                if model.contains("2603") {
                    "low_high"
                } else {
                    "passthrough"
                }
                .to_string(),
            ),
            output_format: Some("reasoning".to_string()),
            effort_levels: None,
        });
    }

    if haystack.contains("kimi") || haystack.contains("moonshot") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    if haystack.contains("glm") || haystack.contains("zhipu") || haystack.contains("z.ai") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    if haystack.contains("qwen") || haystack.contains("dashscope") || haystack.contains("bailian") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("enable_thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    if haystack.contains("minimax") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("reasoning_split".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_details".to_string()),
            effort_levels: None,
        });
    }

    if haystack.contains("mimo") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    None
}

/// 聚合 / 托管平台的 reasoning 接口由平台决定：同一个模型在不同平台参数可能完全不同
/// （DeepSeek 官方用 `thinking:{type}`、SiliconFlow 用 `enable_thinking`、
/// OpenRouter 用原生 `reasoning:{effort}` 对象）。仅以平台标识（name / base_url）判定，
/// 绝不掺入 model 名——model 名属于模型厂商，会把托管平台误判成模型官方接口。
fn infer_aggregator_platform_config(
    name: &str,
    base_url: &str,
) -> Option<CodexChatReasoningConfig> {
    let platform = format!("{name} {base_url}");

    // OpenRouter：用原生归一化对象 `reasoning: { effort }`（由 OpenRouter 翻译成各底层
    // 模型的正确推理参数，比顶层 OpenAI 别名 reasoning_effort 覆盖面更全）。effort 走
    // "openrouter" 值映射：枚举为 xhigh|high|medium|low|minimal，无 max——max 会触发
    // `400 reasoning_effort: Invalid option`（见 openclaw#77350），故钳到 xhigh。
    // 安全降级：不发 `thinking:{type}`（OpenRouter 不认该字段），避免误配导致请求被拒。
    if platform.contains("openrouter") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(false),
            supports_effort: Some(true),
            thinking_param: Some("none".to_string()),
            effort_param: Some("reasoning.effort".to_string()),
            effort_value_mode: Some("openrouter".to_string()),
            output_format: Some("auto".to_string()),
            effort_levels: None,
        });
    }

    // SiliconFlow：平台级统一 `enable_thinking`，思维回传 reasoning_content。
    // 安全降级：不按 reasoning_effort 发 effort（平台用 thinking_budget 控制深度，
    // 发 reasoning_effort 反而可能不被接受）。
    if platform.contains("siliconflow") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("enable_thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    // ModelScope 魔搭 API-Inference：与 SiliconFlow 同构——平台级统一
    // `enable_thinking` 布尔（官方模型页范例 extra_body {"enable_thinking": bool}，
    // OpenAI SDK 的 extra_body 合并进请求体顶层），思维回传 reasoning_content。
    // 智谱风格 thinking:{type} 是模型厂商自家方言，平台文档零出现——没有这条
    // 分支时挂 GLM 的 ModelScope 供应商会被下方 glm 模型规则错误注入该形态。
    if platform.contains("modelscope") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("enable_thinking".to_string()),
            effort_param: Some("none".to_string()),
            effort_value_mode: None,
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    // OpenCode Zen（opencode.ai 网关，issue #6112）：其自家客户端对该传输发顶层
    // `reasoning_effort`（provider/transform.ts），平台归一参数；不发厂商原生
    // thinking 形状（glm 模型走 zen 时套智谱 thinking:{type} 网关不认）。
    // 合法档位逐模型（models.dev 的 reasoning_options，opencode 客户端同样严格
    // 按模型声明发值）：具体档位表见供应商 modelCatalog 各条目的 reasoningLevels，
    // 代理由此按请求模型查表钳制（resolve 处附上 effort_levels），无表不发字段。
    // 匹配域名而非裸 "opencode"，避免误伤名字含 opencode 的无关供应商。
    if platform.contains("opencode.ai") {
        return Some(CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            thinking_param: Some("none".to_string()),
            effort_param: Some("reasoning_effort".to_string()),
            effort_value_mode: Some("zen".to_string()),
            output_format: Some("reasoning_content".to_string()),
            effort_levels: None,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reasoning_without_a_switch_does_not_offer_disabled() {
        let transport = CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("none".into()),
            ..Default::default()
        };
        let capability = resolve("alias", None, None, Some(&transport)).unwrap();
        assert_eq!(capability.levels, vec!["high"]);
        assert_eq!(capability.default_level.as_deref(), Some("high"));
    }

    #[test]
    fn authoritative_reasoning_capabilities_enable_standard_transport() {
        let official = native_capabilities(&json!({
            "supported_reasoning_levels": [{"effort":"low"},{"effort":"high"}],
            "default_reasoning_level":"low"
        }));
        let transport = chat_transport(
            "provider",
            "https://api.example.com",
            "model",
            None,
            &json!({}),
            official,
        )
        .unwrap();
        assert_eq!(transport.supports_effort, Some(true));
        assert_eq!(
            transport.effort_levels,
            Some(vec!["low".into(), "high".into()])
        );
        assert_eq!(transport.thinking_param.as_deref(), Some("none"));
    }
}
