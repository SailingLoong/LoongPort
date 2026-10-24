//! Result of one original account-owner check. No scheduling, storage or credentials.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutomaticResult {
    Completed,
    Failed,
    Skipped,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutomaticReason {
    Locked,
    Disabled,
    Pending,
    NotEligible,
    CheckFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutomaticOutcome {
    pub result: AutomaticResult,
    pub reason: Option<AutomaticReason>,
}
impl AutomaticOutcome {
    pub fn completed() -> Self {
        Self {
            result: AutomaticResult::Completed,
            reason: None,
        }
    }
    pub fn failed() -> Self {
        Self {
            result: AutomaticResult::Failed,
            reason: Some(AutomaticReason::CheckFailed),
        }
    }
    pub fn skipped(reason: AutomaticReason) -> Self {
        Self {
            result: AutomaticResult::Skipped,
            reason: Some(reason),
        }
    }
}
