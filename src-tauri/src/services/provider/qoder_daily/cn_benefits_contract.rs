//! CN free-benefit wire contract; independent of international daily rewards.
use crate::database::CnTarget;
use serde_json::Value;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Claimable,
    Claimed,
    Unknown,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Activity {
    pub(crate) target: CnTarget,
    pub(crate) status: Status,
    pub(crate) amount: Option<f64>,
    pub(crate) skip_reason: Option<&'static str>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseError {
    VerificationRequired,
    InvalidResponse,
    IdentityMismatch,
    ServiceRejected,
    ResponseTooLarge,
}
pub(crate) const MAX_BYTES: usize = 256 * 1024;
fn decode(body: &[u8]) -> Result<Value, ParseError> {
    if body.len() > MAX_BYTES {
        return Err(ParseError::ResponseTooLarge);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| ParseError::InvalidResponse)?;
    if !value.is_object() {
        return Err(ParseError::InvalidResponse);
    }
    Ok(value)
}
fn identity(value: &Value, uid: &str) -> Result<(), ParseError> {
    if value.get("uid").is_some_and(|v| v.as_str() != Some(uid)) {
        Err(ParseError::IdentityMismatch)
    } else {
        Ok(())
    }
}
fn envelope(value: &Value, receipt: bool) -> Result<(), ParseError> {
    for value in std::iter::once(value).chain(value.get("data").filter(|v| v.is_object())) {
        for key in ["code", "status"] {
            if value.get(key).is_some_and(|v| {
                matches!(
                    v.as_str(),
                    Some(
                        "SAME_PERSON_ALREADY_CLAIMED"
                            | "BLOCKED"
                            | "FORBIDDEN"
                            | "CHALLENGE_REQUIRED"
                            | "UNAUTHORIZED"
                            | "AUTHENTICATION_REQUIRED"
                    )
                ) || matches!(v.as_i64(), Some(401 | 403))
            }) {
                return Err(ParseError::VerificationRequired);
            }
        }
    }
    if value
        .get("success")
        .is_some_and(|v| v.as_bool() != Some(true))
        || value.get("error").is_some_and(|v| !v.is_null())
    {
        return Err(ParseError::ServiceRejected);
    }
    for (key, code) in ["code", "status"]
        .into_iter()
        .filter_map(|key| value.get(key).map(|v| (key, v)))
    {
        let code = code
            .as_i64()
            .map(|v| v.to_string())
            .or_else(|| code.as_str().map(str::to_owned));
        if !(matches!(code.as_deref(), Some("0" | "200" | "SUCCESS" | "OK"))
            || receipt && key == "status" && code.as_deref() == Some("CLAIMED"))
        {
            return Err(ParseError::ServiceRejected);
        }
    }
    Ok(())
}
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| {
            !s.trim().is_empty()
                && s.len() <= 256
                && !s.chars().any(|c| {
                    c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
                })
        })
        .map(str::to_owned)
}
fn seconds(value: &Value) -> Option<i64> {
    value.as_i64().filter(|v| *v >= 0 && *v <= 253_402_300_799)
}
fn activity(value: &Value, uid: &str) -> Activity {
    let target = CnTarget {
        campaign_id: value.get("campaignId").and_then(text).unwrap_or_default(),
        campaign_key: value.get("campaignKey").and_then(text),
        start_at: value.get("startAt").and_then(seconds),
        end_at: value.get("endAt").and_then(seconds),
    };
    let malformed = !target.valid()
        || ["campaignKey", "startAt", "endAt"].into_iter().any(|key| {
            value
                .get(key)
                .filter(|v| !v.is_null())
                .is_some_and(|v| match key {
                    "campaignKey" => text(v).is_none(),
                    _ => seconds(v).is_none(),
                })
        });
    let skip_reason = if malformed {
        Some("invalid_activity")
    } else if identity(value, uid).is_err() {
        Some("identity_mismatch")
    } else if value.get("actionType").and_then(Value::as_str) != Some("CLAIM_BENEFIT") {
        Some("unsupported_action")
    } else if value
        .get("unavailableReason")
        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        Some("unavailable")
    } else if value
        .get("requiredAchievementKey")
        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
        && value.get("achievementCompleted").and_then(Value::as_bool) != Some(true)
    {
        Some("achievement_required")
    } else if [
        "purchaseRequired",
        "subscriptionRequired",
        "termsAcceptanceRequired",
    ]
    .into_iter()
    .any(|k| {
        value
            .get(k)
            .is_some_and(|v| !v.is_null() && v.as_bool() != Some(false))
    }) || value
        .get("creditsCost")
        .is_some_and(|v| !v.is_null() && v.as_f64() != Some(0.0))
    {
        Some("additional_condition")
    } else {
        None
    };
    Activity {
        target,
        status: match value.get("claimStatus").and_then(Value::as_str) {
            Some("CLAIMABLE") => Status::Claimable,
            Some("CLAIMED") => Status::Claimed,
            _ => Status::Unknown,
        },
        amount: value
            .pointer("/benefit/amount")
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1_000_000_000.0),
        skip_reason,
    }
}
pub(crate) fn listing(body: &[u8], uid: &str) -> Result<Vec<Activity>, ParseError> {
    let body = decode(body)?;
    envelope(&body, false)?;
    identity(&body, uid)?;
    let rows = body
        .get("campaigns")
        .and_then(Value::as_array)
        .ok_or(ParseError::InvalidResponse)?;
    if rows.len() > 100 {
        return Err(ParseError::ResponseTooLarge);
    }
    let mut items: Vec<_> = rows.iter().map(|v| activity(v, uid)).collect();
    let mut counts = std::collections::HashMap::new();
    for item in &items {
        *counts
            .entry(item.target.campaign_id.clone())
            .or_insert(0usize) += 1;
    }
    for item in &mut items {
        if counts[&item.target.campaign_id] > 1 {
            item.skip_reason = Some("ambiguous_campaign");
        }
    }
    Ok(items)
}
pub(crate) fn claimed(body: &[u8], uid: &str, target: &CnTarget) -> Result<bool, ParseError> {
    let body = decode(body)?;
    envelope(&body, true)?;
    identity(&body, uid)?;
    let data = body.get("data").filter(|v| v.is_object());
    for value in std::iter::once(&body).chain(data) {
        envelope(value, true)?;
        identity(value, uid)?;
        if value
            .get("campaignId")
            .is_some_and(|v| v.as_str() != Some(target.campaign_id.as_str()))
            || target.campaign_key.as_deref().is_some_and(|key| {
                value
                    .get("campaignKey")
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| v.as_str() != Some(key))
            })
            || target.start_at.is_some_and(|start| {
                value
                    .get("startAt")
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| seconds(v) != Some(start))
            })
            || target.end_at.is_some_and(|end| {
                value
                    .get("endAt")
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| seconds(v) != Some(end))
            })
        {
            return Err(ParseError::InvalidResponse);
        }
    }
    Ok(
        body.get("status").and_then(Value::as_str) == Some("CLAIMED")
            || data.and_then(|v| v.get("status")).and_then(Value::as_str) == Some("CLAIMED"),
    )
}
#[cfg(test)]
#[path = "cn_benefits_contract_tests.rs"]
mod tests;
