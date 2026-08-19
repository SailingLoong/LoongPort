use serde::{Deserialize, Serialize};

pub const RULES_VERSION: i32 = 1;

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
