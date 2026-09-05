use serde::{Deserialize, Serialize};

/// 判定规则版本：读侧只认当前版本的报告（store 过滤），升版即作废存量。
/// v2：模型名前缀匹配、官方流式用量语义、证据按码去重、SSE 归一化（2026-09）。
pub const RULES_VERSION: i32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetKey {
    pub provider_id: String,
    pub app_type: String,
    pub model: String,
}

impl TargetKey {
    pub fn new(
        provider_id: impl Into<String>,
        app_type: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            app_type: app_type.into(),
            model: model.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetScope {
    pub provider_id: String,
    pub app_type: String,
}

impl TargetScope {
    pub fn new(provider_id: impl Into<String>, app_type: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            app_type: app_type.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Trusted,
    Suspicious,
    Anomaly,
    Inconclusive,
}

impl Verdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Suspicious => "suspicious",
            Self::Anomaly => "anomaly",
            Self::Inconclusive => "inconclusive",
        }
    }
}

impl TryFrom<&str> for Verdict {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "trusted" => Ok(Self::Trusted),
            "suspicious" => Ok(Self::Suspicious),
            "anomaly" => Ok(Self::Anomaly),
            "inconclusive" => Ok(Self::Inconclusive),
            _ => Err("unsupported verification verdict"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceLevel {
    Cryptographic,
    ProtocolBehavior,
    Insufficient,
}

impl EvidenceLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cryptographic => "cryptographic",
            Self::ProtocolBehavior => "protocolBehavior",
            Self::Insufficient => "insufficient",
        }
    }
}

impl TryFrom<&str> for EvidenceLevel {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "cryptographic" => Ok(Self::Cryptographic),
            "protocolBehavior" => Ok(Self::ProtocolBehavior),
            "insufficient" => Ok(Self::Insufficient),
            _ => Err("unsupported verification evidence level"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceCode {
    BasicEnvelope,
    ModelMatch,
    StreamLifecycle,
    UsageConsistency,
    ToolCallShape,
    StructuredOutput,
    ThinkingSignature,
    SignatureContinuation,
    ForeignProtocol,
    ForeignSelfIdentification,
}

impl EvidenceCode {
    pub const ALL: [Self; 10] = [
        Self::BasicEnvelope,
        Self::ModelMatch,
        Self::StreamLifecycle,
        Self::UsageConsistency,
        Self::ToolCallShape,
        Self::StructuredOutput,
        Self::ThinkingSignature,
        Self::SignatureContinuation,
        Self::ForeignProtocol,
        Self::ForeignSelfIdentification,
    ];

    pub const CARDINALITY: usize = Self::ALL.len();

    pub const fn index(self) -> usize {
        match self {
            Self::BasicEnvelope => 0,
            Self::ModelMatch => 1,
            Self::StreamLifecycle => 2,
            Self::UsageConsistency => 3,
            Self::ToolCallShape => 4,
            Self::StructuredOutput => 5,
            Self::ThinkingSignature => 6,
            Self::SignatureContinuation => 7,
            Self::ForeignProtocol => 8,
            Self::ForeignSelfIdentification => 9,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceOutcome {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceFact {
    pub code: EvidenceCode,
    pub outcome: EvidenceOutcome,
}

/// 一次验证中失败腿的原始请求/响应（debug 边车，不参与判定）。
///
/// 只在对应证据事实 Failed 时采集；thinking/签名腿一律不采集（签名材料
/// 不落盘）。请求体里天然没有凭证（认证走请求头），可安全展示给用户。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeDiagnostic {
    /// 产出该事实的探针腿：core / identity / tool / structured / stream。
    pub probe: String,
    pub code: EvidenceCode,
    /// 发出的完整请求体（JSON，截断至 4KB）。
    pub request: String,
    /// 收到的响应体（流式腿为事件类型序列 + 截断原文，共 8KB 上限）。
    pub response: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReport {
    pub target: TargetKey,
    pub verdict: Verdict,
    pub evidence_level: EvidenceLevel,
    pub facts: Vec<EvidenceFact>,
    pub rules_version: i32,
    pub checked_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerificationSource {
    Active,
    Passive,
}

impl VerificationSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Passive => "passive",
        }
    }
}

/// 判定严重度（数字越大越严重）。主动/被动两源合并与跨模型聚合共用这一把尺。
pub(crate) const fn verdict_severity(verdict: Verdict) -> u8 {
    match verdict {
        Verdict::Trusted => 0,
        Verdict::Inconclusive => 1,
        Verdict::Suspicious => 2,
        Verdict::Anomaly => 3,
    }
}

impl TryFrom<&str> for VerificationSource {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "active" => Ok(Self::Active),
            "passive" => Ok(Self::Passive),
            _ => Err("unsupported verification source"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationHistoryEntry {
    pub source: VerificationSource,
    pub report: VerificationReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub enum RunFailureKind {
    Authentication,
    RateLimited,
    InsufficientBalance,
    Network,
    Upstream,
    Timeout,
    ModelUnavailable,
    Cancelled,
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunResponse {
    pub run_id: String,
    pub state: RunState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationProgressEvent {
    pub run_id: String,
    pub provider_id: String,
    pub app_type: String,
    pub model: String,
    pub state: RunState,
    pub completed_checks: u8,
    pub total_checks: u8,
    pub failure: Option<RunFailureKind>,
}

/// 「这个模型能不能用当前验证协议验」的预筛结论。
///
/// 判据的唯一源是站点 ai-transit 公开快照里逐模型声明的 `supported_protocols`
/// （设计档见 design TODO「模型验证·协议级筛选」）。**只有快照正向覆盖
/// （分组与模型都能定位）才给 Supported/Unsupported**；站点没公开数据、分组
/// 不在快照里（站点只发布部分分组是常态）、模型不在清单里 —— 一律 Unknown，
/// 照常可选。宁可少排除，不误杀。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFitness {
    /// 快照声明该分组下此模型支持当前验证协议。
    Supported,
    /// 快照**正向覆盖**该分组与模型，且协议清单不含当前验证协议。
    UnsupportedProtocol,
    /// 快照没有覆盖到（站点无公开数据 / 分组不在快照 / 模型不在清单）。
    Unknown,
}

/// 验证弹窗的一个模型选项：模型名 + 预筛结论。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationModelOption {
    pub name: String,
    pub fitness: ModelFitness,
}
