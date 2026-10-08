//! Adapted from workbuddy-switch checkin.rs (MIT); see docs/licenses/workbuddy-switch-MIT.txt.
//! No scheduler, automatic claim, raw receipt, token refresh or write retry.
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ClaimState {
    #[default]
    Unconfirmed,
    Available,
    Claimed,
    AlreadyClaimed,
    Unavailable,
    NeedsVerification,
}

pub(crate) fn code(v: &Value) -> Option<i64> {
    let v = v.get("code")?;
    v.as_i64().or_else(|| v.as_str()?.parse().ok())
}
pub(crate) fn success(v: &Value) -> bool {
    matches!(code(v), Some(0 | 200))
}
fn error_state(v: &Value) -> ClaimState {
    match code(v) {
        Some(401 | 403 | 10085) => ClaimState::NeedsVerification,
        Some(1001) => ClaimState::AlreadyClaimed,
        Some(1002 | 1003) => ClaimState::Unavailable,
        _ => ClaimState::Unconfirmed,
    }
}
pub(crate) fn status(v: &Value) -> ClaimState {
    if !success(v) {
        return error_state(v);
    }
    let Some(d) = v.get("data") else {
        return ClaimState::Unconfirmed;
    };
    let checked = d
        .get("today_checked_in")
        .or_else(|| d.get("todayCheckedIn"))
        .and_then(Value::as_bool);
    match (checked, d.get("active").and_then(Value::as_bool)) {
        (Some(true), _) => ClaimState::AlreadyClaimed,
        (Some(false), Some(true)) => ClaimState::Available,
        (Some(false), Some(false)) => ClaimState::Unavailable,
        _ => ClaimState::Unconfirmed,
    }
}
pub(crate) fn receipt(v: &Value) -> ClaimState {
    if success(v) {
        ClaimState::Claimed
    } else {
        error_state(v)
    }
}
pub(crate) fn receipt_credit(v: &Value) -> Option<f64> {
    super::credits::number(v.get("credit").or_else(|| v.get("data")?.get("credit")))
}
pub(crate) fn reconcile(
    receipt: ClaimState,
    after: ClaimState,
    before_balance: Option<f64>,
    after_balance: Option<f64>,
    credit: Option<f64>,
) -> (ClaimState, Option<f64>) {
    if receipt == ClaimState::AlreadyClaimed && after == ClaimState::AlreadyClaimed {
        return (ClaimState::AlreadyClaimed, None);
    }
    if matches!(
        receipt,
        ClaimState::NeedsVerification | ClaimState::Unavailable
    ) {
        return (receipt, None);
    }
    if receipt == ClaimState::Claimed && after == ClaimState::AlreadyClaimed {
        if let (Some(before), Some(after), Some(credit)) = (before_balance, after_balance, credit) {
            if credit > 0.0 && after - before >= credit - 0.000001 {
                return (ClaimState::Claimed, Some(credit));
            }
        }
    }
    (ClaimState::Unconfirmed, None)
}
