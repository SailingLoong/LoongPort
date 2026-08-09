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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

impl VerificationReport {
    pub fn summary(&self) -> VerificationSummary {
        VerificationSummary {
            target: self.target.clone(),
            verdict: self.verdict,
            evidence_level: self.evidence_level,
            rules_version: self.rules_version,
            checked_at: self.checked_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationSummary {
    pub target: TargetKey,
    pub verdict: Verdict,
    pub evidence_level: EvidenceLevel,
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
