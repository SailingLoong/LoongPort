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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceLevel {
    Cryptographic,
    ProtocolBehavior,
    Insufficient,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVerificationSetting {
    pub runtime_auto_enabled: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeAppType {
    Codex,
    Claude,
}

/// Short alias for callers that refer to the supported runtime clients as apps.
pub type RuntimeApp = RuntimeAppType;

impl RuntimeAppType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

impl TryFrom<&str> for RuntimeAppType {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            _ => Err("unsupported runtime app type"),
        }
    }
}

impl TryFrom<String> for RuntimeAppType {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeAppStatus {
    Active,
    Waiting,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeAppReason {
    CurrentProviderUnsupported,
    ClientUnavailable,
    NoCurrentProvider,
    TakeoverFailed,
    RecoveryFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAppState {
    pub app_type: RuntimeAppType,
    pub status: RuntimeAppStatus,
    pub reason: Option<RuntimeAppReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyLease {
    pub app_type: RuntimeAppType,
    pub acquired_at: i64,
}
