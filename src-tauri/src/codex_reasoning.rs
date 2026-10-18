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

/// `Supported` does not assert that thinking can be disabled or is mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningMode {
    #[default]
    Unknown,
    Unsupported,
    Toggle,
    Required,
    Supported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CapabilitySource {
    #[default]
    Unknown,
    AppDeclaration,
    EndpointMetadata,
    ExactProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TransportKind {
    #[default]
    Unknown,
    ExplicitTemplate,
    ExactEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningTransport {
    pub kind: TransportKind,
    pub supports_thinking: Option<bool>,
    pub supports_effort: Option<bool>,
    pub thinking_param: Option<String>,
    pub effort_param: Option<String>,
    pub effort_value_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Default)]
pub struct ReasoningControls {
    pub on: bool,
    pub off: bool,
    pub effort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningCapabilities {
    pub levels: Vec<String>,
    pub default_level: Option<String>,
    pub mode: ReasoningMode,
    pub source: CapabilitySource,
    pub conflicts: Vec<String>,
    /// True only for model facts bound to this endpoint/protocol by a traced contract.
    /// This does not assert a live probe, upstream acceptance, or observed usage.
    pub verified: bool,
    pub transport: ReasoningTransport,
    pub controls: ReasoningControls,
    /// Derived intent constraints, separate from the UI's selectable levels.
    /// None means unknown, never a denial; an empty set needs an explicit hard denial.
    /// This value is never serialized, persisted, or independently supplied by a caller.
    #[serde(skip)]
    request_levels: Option<Vec<String>>,
}

impl ReasoningCapabilities {
    pub(crate) fn from_levels(levels: Vec<String>, default_level: Option<String>) -> Self {
        let levels = canonical_levels(&levels);
        let mode = if levels.iter().any(|level| level != "none") {
            if levels.iter().any(|level| level == "none") {
                ReasoningMode::Toggle
            } else {
                ReasoningMode::Supported
            }
        } else {
            ReasoningMode::Unknown
        };
        let default_level = default_level.filter(|level| levels.contains(level));
        Self {
            request_levels: (!levels.is_empty()).then(|| levels.clone()),
            levels,
            default_level,
            mode,
            ..Self::default()
        }
    }

    /// Codex's legacy catalog represents its off control in the effort list.
    /// This projection adds no synthetic positive effort for toggle-only models.
    pub(crate) fn catalog_levels(&self) -> Vec<String> {
        let mut levels = self.levels.clone();
        if self.mode == ReasoningMode::Toggle && !levels.iter().any(|level| level == "none") {
            levels.insert(0, "none".into());
        }
        levels
    }

    pub(crate) fn validate_intent(&self, intent: &ReasoningIntent) -> Result<(), String> {
        if matches!(intent, ReasoningIntent::ServiceDefault) {
            return Ok(());
        }
        if !self.conflicts.is_empty() {
            return Err(self.conflicts.join("; "));
        }
        if self.transport.kind != TransportKind::Unknown
            || self.transport.supports_effort.is_some()
            || self.transport.supports_thinking.is_some()
            || self.transport.effort_param.is_some()
            || self.transport.thinking_param.is_some()
        {
            validate_transport_intent(
                &CodexChatReasoningConfig {
                    supports_thinking: self.transport.supports_thinking,
                    supports_effort: self.transport.supports_effort,
                    thinking_param: self.transport.thinking_param.clone(),
                    effort_param: self.transport.effort_param.clone(),
                    effort_value_mode: self.transport.effort_value_mode.clone(),
                    effort_levels: self.request_levels.clone(),
                    ..Default::default()
                },
                intent,
            )?;
        }
        match intent {
            ReasoningIntent::ServiceDefault => Ok(()),
            ReasoningIntent::Effort(effort)
                if self.mode != ReasoningMode::Unsupported
                    && self
                        .request_levels
                        .as_ref()
                        .is_none_or(|levels| levels.contains(effort)) =>
            {
                Ok(())
            }
            // Supported says nothing about switchability. Preserve an explicit
            // client off when the protocol can encode it; this grants no UI control.
            ReasoningIntent::Off
                if matches!(
                    self.mode,
                    ReasoningMode::Unknown | ReasoningMode::Toggle | ReasoningMode::Supported
                ) || self.levels.iter().any(|level| level == "none") =>
            {
                Ok(())
            }
            ReasoningIntent::On
                if matches!(
                    self.mode,
                    ReasoningMode::Unknown
                        | ReasoningMode::Toggle
                        | ReasoningMode::Required
                        | ReasoningMode::Supported
                ) =>
            {
                Ok(())
            }
            _ => Err("The selected model does not declare support for this thinking choice".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "level",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum ReasoningIntent {
    ServiceDefault,
    On,
    Off,
    Effort(String),
}

/// `exclude` and summary controls describe output, not whether thinking is enabled.
pub(crate) fn request_intent(body: &Value) -> Result<ReasoningIntent, String> {
    let Some(reasoning) = body.get("reasoning") else {
        return Ok(ReasoningIntent::ServiceDefault);
    };
    if !reasoning.is_null() && !reasoning.is_object() {
        return Err("reasoning must be an object or null".into());
    }
    if reasoning.is_null() {
        return Ok(ReasoningIntent::ServiceDefault);
    }
    let enabled = reasoning
        .get("enabled")
        .map(|value| value.as_bool().ok_or("reasoning.enabled must be a boolean"))
        .transpose()?;
    if let Some(effort) = reasoning.get("effort") {
        if effort.is_null() {
            if enabled.is_some() {
                return Err("A service-default effort cannot also select a thinking switch".into());
            }
            return Ok(ReasoningIntent::ServiceDefault);
        }
        let effort = effort.as_str().ok_or("reasoning.effort must be a string")?;
        if effort.trim().is_empty() {
            return Err("reasoning.effort must not be empty".into());
        }
        let intent = match effort.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => ReasoningIntent::Off,
            "default" => ReasoningIntent::ServiceDefault,
            _ => ReasoningIntent::Effort(effort.to_string()),
        };
        if enabled.is_some_and(|enabled| match &intent {
            ReasoningIntent::Off => enabled,
            ReasoningIntent::Effort(_) => !enabled,
            _ => true,
        }) {
            return Err("reasoning.effort conflicts with reasoning.enabled".into());
        }
        return Ok(intent);
    }
    if let Some(enabled) = reasoning.get("enabled") {
        return enabled
            .as_bool()
            .map(|enabled| {
                if enabled {
                    ReasoningIntent::On
                } else {
                    ReasoningIntent::Off
                }
            })
            .ok_or("reasoning.enabled must be a boolean".into());
    }
    Ok(ReasoningIntent::ServiceDefault)
}

/// The caller supplies the app-local choice; source/model service defaults remain
/// metadata and never become an injected request choice.
pub(crate) fn apply_app_default(body: &mut Value, app_default: Option<&ReasoningIntent>) {
    if !body.is_object() {
        return;
    }
    let has_client_choice = body.get("reasoning").is_some_and(|reasoning| {
        !reasoning.is_object()
            || reasoning.get("effort").is_some()
            || reasoning.get("enabled").is_some()
    });
    if has_client_choice {
        return;
    }
    match app_default {
        Some(ReasoningIntent::Effort(effort)) => {
            body["reasoning"]["effort"] = Value::String(effort.clone())
        }
        Some(ReasoningIntent::Off) => body["reasoning"]["effort"] = Value::String("none".into()),
        Some(ReasoningIntent::On) => body["reasoning"]["enabled"] = Value::Bool(true),
        _ => {}
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
                .is_some_and(|candidate| candidate == model)
        })
}

pub(crate) fn declared_capabilities(row: &Value) -> Option<ReasoningCapabilities> {
    let raw_levels = row
        .get("reasoningLevels")
        .or_else(|| row.get("reasoning_levels"));
    let raw_mode = row
        .get("reasoningMode")
        .or_else(|| row.get("reasoning_mode"));
    if raw_levels.is_none() && raw_mode.is_none() {
        return None;
    }
    let levels = raw_levels
        .and_then(Value::as_array)
        .map(|levels| {
            levels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let default = row
        .get("defaultReasoningLevel")
        .or_else(|| row.get("default_reasoning_level"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut result = ReasoningCapabilities::from_levels(levels, default.clone());
    result.source = CapabilitySource::AppDeclaration;
    if let Some(raw) = raw_levels {
        if !raw.is_array()
            || raw.as_array().is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value.as_str().is_none_or(|value| !EFFORTS.contains(&value)))
            })
        {
            result
                .conflicts
                .push("reasoningLevels must contain only supported effort identifiers".into());
        }
    }
    if let Some(raw) = raw_mode {
        result.mode = match raw.as_str() {
            Some("unknown") => ReasoningMode::Unknown,
            Some("unsupported") => ReasoningMode::Unsupported,
            Some("toggle") => ReasoningMode::Toggle,
            Some("required") => ReasoningMode::Required,
            Some("supported") => ReasoningMode::Supported,
            _ => {
                result
                    .conflicts
                    .push("Unknown reasoningMode declaration".into());
                ReasoningMode::Unknown
            }
        };
    }
    if raw_mode.is_some() && result.mode == ReasoningMode::Unknown && !result.levels.is_empty() {
        result.conflicts.push("Unknown thinking cannot declare known effort levels; use supported, toggle, or required".into());
    }
    if result.mode == ReasoningMode::Unsupported && !result.levels.is_empty() {
        result
            .conflicts
            .push("Unsupported thinking cannot declare effort levels".into());
        result.levels.clear();
        result.default_level = None;
    }
    if result.mode == ReasoningMode::Required && result.levels.iter().any(|level| level == "none") {
        result
            .conflicts
            .push("Required thinking cannot declare an off level".into());
        result.levels.retain(|level| level != "none");
        result.default_level = result.default_level.filter(|level| level != "none");
    }
    if default.is_some() && default != result.default_level {
        result
            .conflicts
            .push("The default reasoning level must be in the declared level set".into());
    }
    result.request_levels = if result.mode == ReasoningMode::Unsupported {
        Some(Vec::new())
    } else if result.mode == ReasoningMode::Unknown || result.levels.is_empty() {
        None
    } else {
        Some(result.levels.clone())
    };
    Some(result)
}

pub(crate) fn native_capabilities(entry: &Value) -> Option<ReasoningCapabilities> {
    let raw_levels = entry.get("supported_reasoning_levels")?.as_array()?;
    let levels = raw_levels;
    let levels = canonical_levels(
        &levels
            .iter()
            .filter_map(|level| level.get("effort").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>(),
    );
    if levels.is_empty() && !raw_levels.is_empty() {
        return None;
    }
    let default_level = entry
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .filter(|value| levels.iter().any(|level| level == value))
        .map(str::to_string);
    let mut capabilities = ReasoningCapabilities::from_levels(levels, default_level);
    capabilities.source = CapabilitySource::EndpointMetadata;
    capabilities.verified = true;
    Some(capabilities)
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
                .is_some_and(|slug| slug == model)
        })
        .and_then(declared_capabilities)
        .map(|mut capability| {
            capability.source = CapabilitySource::ExactProfile;
            capability
        })
}

/// Seed facts from a real versioned official release. The loader compares this
/// version with installed CLI/cache sources without inventing model-family rules.
pub(crate) fn seed_official_model_facts() -> Vec<Value> {
    let catalog: Value = serde_json::from_str(include_str!(
        "resources/codex_curated_reasoning_levels.json"
    ))
    .expect("bundled reasoning capabilities must be valid JSON");
    let Some(rows) = catalog.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let source = catalog.get("sources")?.get(row.get("source")?.as_str()?)?;
            if source.get("authority").and_then(Value::as_str) != Some("openai-codex") {
                return None;
            }
            let model = row.get("model")?.as_str()?;
            let capabilities = declared_capabilities(row)?;
            let levels: Vec<_> = capabilities
                .levels
                .iter()
                .map(|effort| {
                    serde_json::json!({
                        "effort": effort, "description": format!("{effort} reasoning")
                    })
                })
                .collect();
            Some(serde_json::json!({
                "slug": model, "supported_reasoning_levels": levels,
                "default_reasoning_level": capabilities.default_level,
                "loongport_reasoning_source": source
            }))
        })
        .collect()
}

/// Endpoint limits constrain explicit/model evidence; an endpoint vocabulary never
/// establishes a model capability. Explicit empty declarations must remain empty.
pub(crate) fn resolve(
    model: &str,
    declared: Option<ReasoningCapabilities>,
    official: Option<ReasoningCapabilities>,
    transport: Option<&CodexChatReasoningConfig>,
) -> Option<ReasoningCapabilities> {
    let hard_model_facts = official.as_ref().filter(|facts| facts.verified).cloned();
    // Downgrade fresh name-only facts once, before applying endpoint constraints.
    // A previous resolved result may enter through `declared` on the transport
    // projection pass; its derived restrictions must survive that second pass.
    let scope_fresh_facts = |mut facts: ReasoningCapabilities| {
        if facts.source == CapabilitySource::ExactProfile && !facts.verified {
            facts.mode = ReasoningMode::Unknown;
            facts.levels.clear();
            facts.default_level = None;
            facts.request_levels = None;
        }
        facts
    };
    let mut capability = declared
        .or_else(|| official.map(scope_fresh_facts))
        .or_else(|| curated_capabilities(model).map(scope_fresh_facts))?;
    if let Some(hard_levels) = hard_model_facts
        .as_ref()
        .and_then(|facts| facts.request_levels.as_ref())
    {
        capability.request_levels = Some(match capability.request_levels.take() {
            Some(levels) => levels
                .into_iter()
                .filter(|level| hard_levels.contains(level))
                .collect(),
            None => hard_levels.clone(),
        });
    }
    if capability.source == CapabilitySource::AppDeclaration {
        if let Some(hard) = hard_model_facts {
            if hard.mode == ReasoningMode::Required {
                if matches!(
                    capability.mode,
                    ReasoningMode::Toggle | ReasoningMode::Unsupported
                ) || capability.levels.iter().any(|level| level == "none")
                {
                    capability.conflicts.push(
                        "Current endpoint requires thinking; this declaration cannot disable it"
                            .into(),
                    );
                }
                capability.mode = ReasoningMode::Required;
            } else if hard.mode == ReasoningMode::Unsupported {
                if matches!(
                    capability.mode,
                    ReasoningMode::Toggle | ReasoningMode::Required | ReasoningMode::Supported
                ) || !capability.levels.is_empty()
                {
                    capability.conflicts.push(
                        "Current endpoint explicitly does not support thinking for this model"
                            .into(),
                    );
                }
                capability.mode = ReasoningMode::Unsupported;
            }
            for level in capability
                .levels
                .iter()
                .filter(|level| level.as_str() != "none")
            {
                if hard
                    .request_levels
                    .as_ref()
                    .is_some_and(|levels| !levels.contains(level))
                {
                    capability.conflicts.push(format!(
                        "Current endpoint model metadata does not support declared effort {level}"
                    ));
                }
            }
        }
    }
    capability.levels = canonical_levels(&capability.levels);
    capability.default_level = capability
        .default_level
        .filter(|level| capability.levels.contains(level));
    let Some(transport) = transport else {
        return Some(capability);
    };
    capability.transport = ReasoningTransport {
        kind: transport
            .transport_kind
            .unwrap_or(TransportKind::ExplicitTemplate),
        supports_thinking: transport.supports_thinking,
        supports_effort: transport.supports_effort,
        thinking_param: transport.thinking_param.clone(),
        effort_param: transport.effort_param.clone(),
        effort_value_mode: transport.effort_value_mode.clone(),
    };
    // Only concrete transport limits create an empty request set. Empty UI
    // choices or an unknown declaration never mean a forbidden client effort.
    let transport_levels = if transport.supports_effort == Some(false) {
        Some(Vec::new())
    } else if let Some(levels) = &transport.effort_levels {
        Some(levels.clone())
    } else if matches!(
        transport.effort_value_mode.as_deref(),
        Some("deepseek" | "low_high" | "openrouter")
    ) {
        Some(
            EFFORTS
                .iter()
                .filter(|level| transport_allows_effort(transport, level))
                .map(|level| (*level).to_string())
                .collect(),
        )
    } else {
        None
    };
    if let Some(limits) = transport_levels {
        capability.request_levels = Some(match capability.request_levels.take() {
            Some(levels) => levels
                .into_iter()
                .filter(|level| limits.contains(level))
                .collect(),
            None => limits,
        });
    }
    let before = capability.levels.clone();
    capability
        .levels
        .retain(|level| transport_allows_effort(transport, level));
    for level in before
        .iter()
        .filter(|level| !capability.levels.contains(level))
    {
        if capability.source == CapabilitySource::AppDeclaration {
            capability.conflicts.push(format!(
                "Endpoint does not support declared reasoning effort {level}"
            ));
        }
    }
    capability.default_level = capability
        .default_level
        .filter(|level| capability.levels.contains(level));
    if capability.mode == ReasoningMode::Toggle && !transport_can_disable(transport) {
        if capability.source == CapabilitySource::AppDeclaration {
            capability
                .conflicts
                .push("Endpoint cannot encode the declared thinking switch".into());
        }
        // Do not misreport mandatory thinking just because the switch is unavailable.
        capability.mode = ReasoningMode::Supported;
    }
    if matches!(
        capability.mode,
        ReasoningMode::Toggle | ReasoningMode::Required | ReasoningMode::Supported
    ) && transport.supports_thinking == Some(false)
        && transport.supports_effort == Some(false)
    {
        capability
            .conflicts
            .push("Endpoint explicitly does not support thinking".into());
        capability.mode = ReasoningMode::Unsupported;
    }
    if capability.conflicts.is_empty()
        && (capability.source != CapabilitySource::ExactProfile || capability.verified)
    {
        capability.controls.on = matches!(
            capability.mode,
            ReasoningMode::Toggle | ReasoningMode::Required | ReasoningMode::Supported
        ) && capability.validate_intent(&ReasoningIntent::On).is_ok()
            && validate_transport_intent(transport, &ReasoningIntent::On).is_ok();
        capability.controls.off = (capability.mode == ReasoningMode::Toggle
            || capability.levels.iter().any(|level| level == "none"))
            && capability.validate_intent(&ReasoningIntent::Off).is_ok()
            && validate_transport_intent(transport, &ReasoningIntent::Off).is_ok();
        capability.controls.effort = transport.supports_effort == Some(true)
            && capability
                .levels
                .iter()
                .any(|level| level != "none" && transport_allows_effort(transport, level));
    }
    Some(capability)
}

pub(crate) fn resolve_settings_capability(
    model: &str,
    settings: &Value,
    endpoint_metadata: Option<ReasoningCapabilities>,
    transport: Option<&CodexChatReasoningConfig>,
) -> ReasoningCapabilities {
    let mut capability = resolve(
        model,
        model_row(settings, model).and_then(declared_capabilities),
        endpoint_metadata,
        transport,
    )
    .unwrap_or_default();
    if let Some(transport) = transport {
        capability.transport = ReasoningTransport {
            kind: transport
                .transport_kind
                .unwrap_or(TransportKind::ExplicitTemplate),
            supports_thinking: transport.supports_thinking,
            supports_effort: transport.supports_effort,
            thinking_param: transport.thinking_param.clone(),
            effort_param: transport.effort_param.clone(),
            effort_value_mode: transport.effort_value_mode.clone(),
        };
    }
    capability
}

/// One endpoint vocabulary is shared by catalog restrictions and wire encoding.
/// Unknown support is not a hard prohibition on an existing client field. UI
/// controls additionally require explicit supports_effort=true; absence never grants it.
pub(crate) fn transport_allows_effort(config: &CodexChatReasoningConfig, effort: &str) -> bool {
    if effort == "none" {
        return transport_can_disable(config);
    }
    if config.supports_effort == Some(false)
        || config
            .effort_levels
            .as_ref()
            .is_some_and(|levels| !levels.iter().any(|level| level == effort))
    {
        return false;
    }
    let allowed = match config.effort_value_mode.as_deref() {
        // Official Chat contract: https://api-docs.deepseek.com/api/create-chat-completion/
        Some("deepseek") => &["low", "high", "max"][..],
        Some("low_high") => &["low", "high"][..],
        Some("openrouter") => &["minimal", "low", "medium", "high", "xhigh"][..],
        _ => return !effort.trim().is_empty(),
    };
    allowed.contains(&effort)
}

pub(crate) fn transport_can_disable(config: &CodexChatReasoningConfig) -> bool {
    (config.supports_thinking == Some(true)
        && matches!(
            config.thinking_param.as_deref(),
            Some("thinking" | "enable_thinking" | "reasoning_split")
        ))
        || (config.supports_effort == Some(true)
            && matches!(
                config.effort_param.as_deref(),
                Some("reasoning_effort" | "reasoning.effort")
            ))
}

/// Fix saved contradictory enabled+none states without erasing disabled mappings.
pub(crate) fn normalize_chat_transport(
    mut config: CodexChatReasoningConfig,
) -> CodexChatReasoningConfig {
    if config.supports_effort == Some(true)
        && config
            .effort_param
            .as_deref()
            .is_none_or(|param| param.trim().is_empty() || param == "none")
    {
        config.effort_param = Some("reasoning_effort".into());
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReasoningTemplate {
    pub id: &'static str,
    pub label: &'static str,
}

/// IDs select the existing metadata representation, not a second writable DTO.
pub(crate) fn reasoning_templates() -> Vec<ReasoningTemplate> {
    [
        ("openaiChatEffort", "Chat reasoning_effort"),
        ("deepseekChat", "DeepSeek Chat contract"),
        ("openRouter", "OpenRouter reasoning object"),
        ("thinkingToggle", "thinking.type switch"),
        ("enableThinkingToggle", "enable_thinking switch"),
    ]
    .into_iter()
    .map(|(id, label)| ReasoningTemplate { id, label })
    .collect()
}

pub(crate) fn reasoning_template(id: &str) -> Option<CodexChatReasoningConfig> {
    let (thinking, effort, mode) = match id {
        "openaiChatEffort" => ("none", "reasoning_effort", "passthrough"),
        "deepseekChat" => ("thinking", "reasoning_effort", "deepseek"),
        "openRouter" => ("none", "reasoning.effort", "openrouter"),
        "thinkingToggle" => ("thinking", "none", "passthrough"),
        "enableThinkingToggle" => ("enable_thinking", "none", "passthrough"),
        _ => return None,
    };
    Some(CodexChatReasoningConfig {
        supports_thinking: Some(thinking != "none"),
        supports_effort: Some(effort != "none"),
        thinking_param: Some(thinking.into()),
        effort_param: Some(effort.into()),
        effort_value_mode: Some(mode.into()),
        transport_kind: Some(TransportKind::ExplicitTemplate),
        ..Default::default()
    })
}

pub(crate) fn reasoning_template_id(config: &CodexChatReasoningConfig) -> Option<&'static str> {
    reasoning_templates().into_iter().find_map(|template| {
        let candidate = reasoning_template(template.id)?;
        (candidate.supports_thinking == config.supports_thinking
            && candidate.supports_effort == config.supports_effort
            && candidate.thinking_param == config.thinking_param
            && candidate.effort_param == config.effort_param
            && candidate.effort_value_mode == config.effort_value_mode)
            .then_some(template.id)
    })
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
    let endpoint = infer_chat_transport(name, base_url, model);
    let mut config = explicit
        .cloned()
        .map(normalize_chat_transport)
        .or_else(|| endpoint.clone())
        .or_else(|| {
            capabilities
                .as_ref()
                .filter(|capability| capability.levels.iter().any(|level| level != "none"))
                .and_then(|_| reasoning_template("openaiChatEffort"))
        })?;
    if config.transport_kind.is_none() {
        config.transport_kind = Some(TransportKind::ExplicitTemplate);
    }
    // Model declarations can narrow or document a template, never bypass known
    // endpoint limits. Validate that intersection before deriving catalog/wire controls.
    let mut capability = resolve(model, declared, official, endpoint.as_ref());
    capability = resolve(model, capability, None, Some(&config));
    if explicit.is_some() {
        if let Some(endpoint) = &endpoint {
            let wrong_switch = (config.supports_thinking == Some(true)
                || endpoint.supports_thinking == Some(true))
                && (config.supports_thinking != endpoint.supports_thinking
                    || config.thinking_param != endpoint.thinking_param);
            let wrong_effort = config.supports_effort == Some(true)
                && config.effort_param != endpoint.effort_param;
            if wrong_switch || wrong_effort {
                capability.get_or_insert_with(ReasoningCapabilities::default).conflicts
                    .push("Selected compatibility template conflicts with the known endpoint contract".into());
            }
        }
    }
    // Runtime limits are not advertised as model capabilities. Unknown aliases
    // may retain an explicit request, subject to the exact endpoint vocabulary.
    config.effort_levels = capability
        .as_ref()
        .and_then(|capability| capability.request_levels.clone());
    if let Some(endpoint) = &endpoint {
        if matches!(
            endpoint.effort_value_mode.as_deref(),
            Some("deepseek" | "openrouter")
        ) {
            let levels = config
                .effort_levels
                .get_or_insert_with(|| EFFORTS.iter().map(|level| (*level).into()).collect());
            levels.retain(|level| transport_allows_effort(endpoint, level));
        }
    }
    if let Some(capability) = &mut capability {
        if !capability.conflicts.is_empty() {
            capability.controls = ReasoningControls::default();
        }
    }
    config.resolved_capabilities = capability;
    Some(config)
}

/// Native Responses has its own contract; reuse only the exact endpoint limits,
/// never the Chat serialization fields. Other native endpoints remain unknown.
/// Contract: https://api-docs.deepseek.com/api/create-response/
/// `reasoning.effort`: none/low/high/max; absent follows the model's default.
pub(crate) fn native_response_limits(base_url: &str) -> Option<CodexChatReasoningConfig> {
    let mut limits = infer_chat_transport("", base_url, "")?;
    if limits.effort_value_mode.as_deref() != Some("deepseek") {
        return None;
    }
    limits.supports_thinking = Some(false);
    limits.thinking_param = Some("none".into());
    limits.effort_param = Some("reasoning.effort".into());
    Some(limits)
}

pub(crate) fn validate_transport_intent(
    config: &CodexChatReasoningConfig,
    intent: &ReasoningIntent,
) -> Result<(), String> {
    match intent {
        ReasoningIntent::ServiceDefault => Ok(()),
        ReasoningIntent::Effort(level) if transport_allows_effort(config, level) => Ok(()),
        ReasoningIntent::Off if transport_can_disable(config) => Ok(()),
        ReasoningIntent::On if (config.supports_thinking == Some(true) && matches!(config.thinking_param.as_deref(), Some("thinking" | "enable_thinking" | "reasoning_split"))) || config.effort_value_mode.as_deref() == Some("openrouter") => Ok(()),
        _ => Err("Endpoint cannot encode this reasoning choice exactly; choose a supported effort or service default".into()),
    }
}

pub(crate) fn infer_chat_transport(
    _name: &str,
    base_url: &str,
    _model: &str,
) -> Option<CodexChatReasoningConfig> {
    // Endpoint identity, never a display/billing/model name, selects a dialect.
    let url = url::Url::parse(base_url).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.port_or_known_default() != Some(443) || url.query().is_some() || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    let (thinking_param, effort_param, mode, output) = match url.host_str()? {
        "api.deepseek.com" if matches!(path, "" | "/v1" | "/beta") => (
            "thinking",
            "reasoning_effort",
            "deepseek",
            "reasoning_content",
        ),
        "openrouter.ai" if path == "/api/v1" => ("none", "reasoning.effort", "openrouter", "auto"),
        "opencode.ai" if matches!(path, "/zen/v1" | "/zen/go/v1") => {
            ("none", "reasoning_effort", "zen", "reasoning_content")
        }
        _ => return None,
    };
    Some(CodexChatReasoningConfig {
        supports_thinking: Some(thinking_param != "none"),
        supports_effort: Some(true),
        thinking_param: Some(thinking_param.into()),
        effort_param: Some(effort_param.into()),
        effort_value_mode: Some(mode.into()),
        output_format: Some(output.into()),
        transport_kind: Some(TransportKind::ExactEndpoint),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gpt_family_aliases_require_model_specific_reasoning_facts() {
        for model in ["gpt-6", "gpt-5.6", "gpt-future"] {
            assert!(resolve(model, None, None, None).is_none(), "{model}");
            let declared = declared_capabilities(&json!({
                "reasoningLevels":["low","high","ultra"],
                "defaultReasoningLevel":"high"
            }));
            let known = resolve(model, declared, None, None).unwrap();
            assert_eq!(known.levels, vec!["low", "high", "ultra"]);
        }
    }

    #[test]
    fn gateway_effort_vocabulary_does_not_declare_unknown_model_capabilities() {
        let transport =
            infer_chat_transport("OpenRouter", "https://openrouter.ai/api/v1", "gpt-6").unwrap();
        assert!(resolve("gpt-6", None, None, Some(&transport)).is_none());
    }

    #[test]
    fn reasoning_without_a_switch_does_not_offer_disabled() {
        let transport = CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(false),
            thinking_param: Some("none".into()),
            ..Default::default()
        };
        assert!(resolve("alias", None, None, Some(&transport)).is_none());
        let declared =
            declared_capabilities(&json!({"reasoningMode":"required", "reasoningLevels":[]}));
        let capability = resolve("alias", declared, None, Some(&transport)).unwrap();
        assert_eq!(capability.mode, ReasoningMode::Required);
        assert!(capability.levels.is_empty());
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
    #[test]
    fn explicit_empty_is_not_missing_and_must_not_fall_back_to_curated() {
        let explicit = super::declared_capabilities(&json!({"reasoningLevels": []}));
        assert!(explicit.is_some(), "explicit empty declaration lost");
        assert!(super::resolve("gpt-6-astra", explicit, None, None)
            .unwrap()
            .levels
            .is_empty());
    }
    #[test]
    fn changing_model_must_not_silently_raise_low_to_default_high() {
        let capability = super::declared_capabilities(
            &json!({"reasoningLevels": ["high","max"], "defaultReasoningLevel":"high"}),
        )
        .unwrap();
        assert!(capability
            .validate_intent(&ReasoningIntent::Effort("low".into()))
            .is_err());
    }
    #[test]
    fn model_or_display_name_is_not_endpoint_protocol_evidence() {
        for (name, model) in [
            ("openai group", "deepseek-v4.1-flash"),
            ("DeepSeek account", "alias"),
            ("OpenRouter plan", "alias"),
        ] {
            assert!(
                super::infer_chat_transport(name, "https://relay.example/v1", model).is_none(),
                "{name} {model}"
            );
        }
    }
    #[test]
    fn endpoint_lookalikes_do_not_activate_native_dialects() {
        assert!(super::infer_chat_transport(
            "account",
            "https://api.deepseek.com.evil.example/v1",
            "alias"
        )
        .is_none());
    }
    #[test]
    fn explicit_empty_declaration_survives_transport_resolution() {
        let transport = CodexChatReasoningConfig {
            supports_effort: Some(false),
            ..Default::default()
        };
        assert!(
            super::resolve("alias", None, None, Some(&transport)).is_none(),
            "no effort support does not prove model supports an off switch"
        );
    }

    #[test]
    fn explicit_null_blocks_app_default_without_becoming_off() {
        let mut body = json!({"reasoning":null});
        apply_app_default(&mut body, Some(&ReasoningIntent::Effort("max".into())));
        assert_eq!(body, json!({"reasoning":null}));
        assert_eq!(
            request_intent(&body).unwrap(),
            ReasoningIntent::ServiceDefault
        );
    }

    #[test]
    fn name_only_profile_is_unverified_and_never_limits_a_relay_alias() {
        let capability = resolve_settings_capability("deepseek-v4-flash", &json!({}), None, None);
        assert_eq!(capability.mode, ReasoningMode::Unknown);
        assert!(capability.levels.is_empty());
        assert!(!capability.verified);
        assert!(capability
            .validate_intent(&ReasoningIntent::Effort("high".into()))
            .is_ok());
        assert!(!capability.controls.effort);
    }

    #[test]
    fn native_deepseek_uses_responses_contract_without_chat_fields() {
        let native = native_response_limits("https://api.deepseek.com/v1").unwrap();
        assert_eq!(native.effort_param.as_deref(), Some("reasoning.effort"));
        assert_eq!(native.thinking_param.as_deref(), Some("none"));
        for level in ["low", "high", "max"] {
            assert!(
                validate_transport_intent(&native, &ReasoningIntent::Effort(level.into())).is_ok()
            );
        }
        assert!(
            validate_transport_intent(&native, &ReasoningIntent::Effort("medium".into())).is_err()
        );
        assert!(validate_transport_intent(&native, &ReasoningIntent::On).is_err());
        assert!(validate_transport_intent(&native, &ReasoningIntent::Off).is_ok());
        assert!(native_response_limits("https://api.deepseek.com.evil.example/v1").is_none());
    }

    #[test]
    fn controls_are_the_final_intersection_not_independent_ui_booleans() {
        let declaration = declared_capabilities(
            &json!({"reasoningMode":"toggle","reasoningLevels":["low","high"]}),
        );
        let native = native_response_limits("https://api.deepseek.com/v1").unwrap();
        let capability = resolve("alias", declaration.clone(), None, Some(&native)).unwrap();
        assert!(!capability.controls.on);
        assert!(capability.controls.off);
        assert!(capability.controls.effort);
        assert!(!capability.verified);
        let chat = infer_chat_transport("", "https://api.deepseek.com/v1", "").unwrap();
        let capability = resolve("alias", declaration, None, Some(&chat)).unwrap();
        assert!(capability.controls.on && capability.controls.off);
        assert_eq!(
            serde_json::to_value(capability).unwrap()["transport"]["kind"],
            "exactEndpoint"
        );
    }

    #[test]
    fn manual_transport_cannot_widen_a_known_endpoint_contract() {
        let explicit = CodexChatReasoningConfig {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            thinking_param: Some("thinking".into()),
            effort_param: Some("reasoning_effort".into()),
            effort_value_mode: Some("passthrough".into()),
            ..Default::default()
        };
        let settings = json!({"modelCatalog":{"models":[{"model":"alias","reasoningLevels":["low","ultra"]}]}});
        let config = chat_transport(
            "billing group",
            "https://api.deepseek.com/v1",
            "alias",
            Some(&explicit),
            &settings,
            None,
        )
        .unwrap();
        assert!(!config
            .resolved_capabilities
            .as_ref()
            .unwrap()
            .conflicts
            .is_empty());
        assert!(
            validate_transport_intent(&config, &ReasoningIntent::Effort("ultra".into())).is_err()
        );
    }

    #[test]
    fn verified_model_hard_limits_override_contradictory_manual_declarations() {
        let mut hard =
            declared_capabilities(&json!({"reasoningMode":"required","reasoningLevels":["high"]}))
                .unwrap();
        hard.source = CapabilitySource::EndpointMetadata;
        hard.verified = true;
        let declaration = declared_capabilities(
            &json!({"reasoningMode":"toggle","reasoningLevels":["low","high"]}),
        );
        let resolved = resolve("alias", declaration, Some(hard), None).unwrap();
        assert_eq!(resolved.mode, ReasoningMode::Required);
        assert!(!resolved.conflicts.is_empty());
        assert!(resolved.validate_intent(&ReasoningIntent::Off).is_err());
    }

    #[test]
    fn malformed_client_reasoning_never_panics_or_gains_an_app_default() {
        let mut body = json!({"reasoning":"invalid"});
        apply_app_default(&mut body, Some(&ReasoningIntent::Effort("high".into())));
        assert_eq!(body["reasoning"], "invalid");
        assert!(request_intent(&body).is_err());
    }
    #[test]
    fn explicit_unknown_empty_blocks_choices_not_client_intent_or_endpoint_limits() {
        let declaration = declared_capabilities(
            &serde_json::json!({"reasoningMode":"unknown","reasoningLevels":[]}),
        );
        let transport = CodexChatReasoningConfig {
            supports_effort: Some(true),
            effort_param: Some("reasoning.effort".into()),
            effort_value_mode: Some("passthrough".into()),
            ..Default::default()
        };
        let unknown = resolve("alias", declaration.clone(), None, Some(&transport)).unwrap();
        assert_eq!(unknown.mode, ReasoningMode::Unknown);
        assert!(unknown.levels.is_empty());
        assert!(!unknown.controls.on && !unknown.controls.off && !unknown.controls.effort);
        assert!(unknown.validate_intent(&ReasoningIntent::Off).is_ok());
        assert!(unknown
            .validate_intent(&ReasoningIntent::Effort("high".into()))
            .is_ok());
        let hard = native_capabilities(
            &serde_json::json!({"supported_reasoning_levels":[{"effort":"low"},{"effort":"high"}]}),
        );
        let restricted = resolve("alias", declaration, hard, Some(&transport)).unwrap();
        assert!(restricted.levels.is_empty());
        assert!(restricted
            .validate_intent(&ReasoningIntent::Effort("low".into()))
            .is_ok());
        assert!(restricted
            .validate_intent(&ReasoningIntent::Effort("ultra".into()))
            .is_err());
        assert!(restricted.validate_intent(&ReasoningIntent::Off).is_ok());
    }
}
