//! Explicit CN campaign listing. This module never establishes identity or claims rewards.
//! Wire facts: magpie-community/plugins 08ff18a43853ccf23b30fc6dd39b3f10591331ee,
//! packages/qoder/index.mjs:1658-1665, 1984-2061 (MIT). This intentionally does
//! not copy its auto-refresh, runtime lookup, daily selection, or claim behavior.
use super::http::{CredentialSource, PatDeclaration};
use crate::{database::Database, error::AppError};
use serde::Serialize;
use serde_json::Value;

const MAX_BYTES: usize = 256 * 1024;
const MAX_CAMPAIGNS: usize = 100;
#[derive(Debug, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ReadResult {
    Observed {
        provider_id: String,
        credential_generation: String,
        source: CredentialSource,
        campaigns: Vec<Campaign>,
        automatic_claim_enabled: bool,
        blocked_reason: &'static str,
        runtime_identity: &'static str,
    },
    Failed {
        code: FailureCode,
    },
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Campaign {
    campaign_id: String,
    campaign_key: Option<String>,
    claim_status: ClaimStatus,
    start_at: Option<i64>,
    end_at: Option<i64>,
    benefit_amount: Option<f64>,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ClaimStatus {
    Claimable,
    Claimed,
    Unknown,
}
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCode {
    InvalidSource,
    SourceChanged,
    OwnerChanged,
    OwnerUnavailable,
    IdentityRequired,
    IdentityPending,
    IdentityUnknown,
    IdentityExpired,
    IdentityMismatch,
    AuthenticationRequired,
    VerificationRequired,
    RateLimited,
    RedirectRejected,
    HttpError,
    ServiceRejected,
    InvalidResponse,
    ResponseTooLarge,
    Timeout,
    NetworkError,
}
const URL: &str = "https://openapi.qoder.com.cn/sash/api/v1/me/campaigns";
struct Transport {
    client: reqwest::Client,
    url: reqwest::Url,
}
impl Transport {
    fn production() -> Result<Self, FailureCode> {
        Ok(Self {
            client: crate::services::provider::qoder_auth_http::client_builder()
                .build()
                .map_err(|_| FailureCode::NetworkError)?,
            url: reqwest::Url::parse(URL).map_err(|_| FailureCode::NetworkError)?,
        })
    }
    #[cfg(test)]
    fn synthetic(url: reqwest::Url, timeout: std::time::Duration) -> Self {
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        Self {
            client: crate::services::provider::qoder_auth_http::client_builder()
                .no_proxy()
                .timeout(timeout)
                .build()
                .unwrap(),
            url,
        }
    }
}
pub(crate) async fn read(db: &Database, declaration: PatDeclaration) -> ReadResult {
    let transport = match Transport::production() {
        Ok(transport) => transport,
        Err(code) => return ReadResult::Failed { code },
    };
    read_using(db, declaration, &transport, || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(-1)
    })
    .await
}
async fn read_using(
    db: &Database,
    declaration: PatDeclaration,
    transport: &Transport,
    now: impl Fn() -> i64,
) -> ReadResult {
    if !matches!(
        declaration.source,
        CredentialSource::ChinaQoderPat | CredentialSource::ChinaQoderDevice
    ) {
        return ReadResult::Failed {
            code: FailureCode::InvalidSource,
        };
    }
    let snapshot = match db.qoder_http_read_snapshot(&declaration, now()) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return ReadResult::Failed {
                code: owner_failure(&error),
            }
        }
    };
    // The declaration is not identity. The original encrypted owner independently
    // derives and checks the current Provider source before returning this snapshot.
    if snapshot.region() != "cn" || snapshot.stamp.source.origin != "https://openapi.qoder.com.cn" {
        return ReadResult::Failed {
            code: FailureCode::InvalidSource,
        };
    }
    let request = match transport
        .client
        .get(transport.url.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("Cosy-ClientType", "10")
        .header(reqwest::header::USER_AGENT, "Qoder")
        .bearer_auth(snapshot.access_token())
        .build()
    {
        Ok(request) => request,
        Err(_) => {
            return ReadResult::Failed {
                code: FailureCode::NetworkError,
            }
        }
    };
    if let Err(error) = db.check_qoder_http_snapshot(&snapshot, now()) {
        return ReadResult::Failed {
            code: owner_failure(&error),
        };
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut response = transport
            .client
            .execute(request)
            .await
            .map_err(network_failure)?;
        if response.url() != &transport.url {
            return Err(FailureCode::RedirectRejected);
        }
        match response.status().as_u16() {
            200 => {}
            300..=399 => return Err(FailureCode::RedirectRejected),
            401 => return Err(FailureCode::AuthenticationRequired),
            403 => return Err(FailureCode::VerificationRequired),
            429 => return Err(FailureCode::RateLimited),
            _ => return Err(FailureCode::HttpError),
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(FailureCode::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network_failure)? {
            if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
                return Err(FailureCode::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_listing(&bytes, snapshot.uid())
    })
    .await
    .unwrap_or(Err(FailureCode::Timeout));
    // Even a valid listing cannot survive a lock/epoch/source/owner change or
    // natural expiry during the GET. No refresh, rejection write or rebind here.
    if let Err(error) = db.check_qoder_http_snapshot(&snapshot, now()) {
        return ReadResult::Failed {
            code: owner_failure(&error),
        };
    }
    match result {
        Ok(campaigns) => ReadResult::Observed {
            provider_id: snapshot.stamp.source.provider_id.clone(),
            credential_generation: snapshot.stamp.source.credential_generation.clone(),
            source: snapshot.stamp.source.source.declaration(),
            campaigns,
            automatic_claim_enabled: false,
            blocked_reason: "daily_contract_unverified",
            runtime_identity: "not_requested",
        },
        Err(code) => ReadResult::Failed { code },
    }
}
fn network_failure(error: reqwest::Error) -> FailureCode {
    if error.is_timeout() {
        FailureCode::Timeout
    } else {
        FailureCode::NetworkError
    }
}
fn owner_failure(error: &AppError) -> FailureCode {
    match error {
        AppError::Config(code) => match code.strip_prefix("qoder.http_auth.") {
            Some("source_changed" | "source_declaration_required" | "selected_source_changed") => {
                FailureCode::SourceChanged
            }
            Some("stale_owner") => FailureCode::OwnerChanged,
            Some(
                "unsupported_source"
                | "external_source_untracked"
                | "source_generation_required"
                | "source_required"
                | "device_source_has_api_key",
            ) => FailureCode::InvalidSource,
            Some("identity_required") => FailureCode::IdentityRequired,
            Some("attempt_in_progress") => FailureCode::IdentityPending,
            Some("outcome_unknown_reestablish_required") => FailureCode::IdentityUnknown,
            Some("snapshot_expired") => FailureCode::IdentityExpired,
            _ => FailureCode::OwnerUnavailable,
        },
        _ => FailureCode::OwnerUnavailable,
    }
}
fn parse_listing(body: &[u8], uid: &str) -> Result<Vec<Campaign>, FailureCode> {
    if body.len() > MAX_BYTES {
        return Err(FailureCode::ResponseTooLarge);
    }
    let body: Value = serde_json::from_slice(body).map_err(|_| FailureCode::InvalidResponse)?;
    let object = body.as_object().ok_or(FailureCode::InvalidResponse)?;
    // An explicit failure envelope must not be presented as an empty success.
    for code in [object.get("code"), object.get("status")]
        .into_iter()
        .flatten()
    {
        match code
            .as_i64()
            .map(|n| n.to_string())
            .or_else(|| code.as_str().map(str::to_owned))
            .as_deref()
        {
            Some("0" | "200" | "SUCCESS" | "OK") => {}
            Some("401" | "UNAUTHORIZED") => return Err(FailureCode::AuthenticationRequired),
            Some("403" | "FORBIDDEN") => return Err(FailureCode::VerificationRequired),
            Some("429" | "RATE_LIMITED") => return Err(FailureCode::RateLimited),
            _ => return Err(FailureCode::ServiceRejected),
        }
    }
    if object
        .get("success")
        .is_some_and(|v| v.as_bool() != Some(true))
        || object.get("error").is_some_and(|v| !v.is_null())
    {
        return Err(FailureCode::ServiceRejected);
    }
    if object.get("uid").is_some_and(|v| v.as_str() != Some(uid)) {
        return Err(FailureCode::IdentityMismatch);
    }
    let rows = object
        .get("campaigns")
        .and_then(Value::as_array)
        .ok_or(FailureCode::InvalidResponse)?;
    if rows.len() > MAX_CAMPAIGNS {
        return Err(FailureCode::ResponseTooLarge);
    }
    let mut ids = std::collections::HashSet::new();
    let mut campaigns = Vec::new();
    for row in rows {
        let row = row.as_object().ok_or(FailureCode::InvalidResponse)?;
        let action = row
            .get("actionType")
            .and_then(Value::as_str)
            .ok_or(FailureCode::InvalidResponse)?;
        if action != "CLAIM_BENEFIT" {
            continue;
        }
        let id = row
            .get("campaignId")
            .and_then(bounded_text)
            .ok_or(FailureCode::InvalidResponse)?;
        if !ids.insert(id.clone()) {
            return Err(FailureCode::InvalidResponse);
        }
        let mut start_at = row.get("startAt").and_then(timestamp_ms);
        let mut end_at = row.get("endAt").and_then(timestamp_ms);
        if matches!((start_at, end_at), (Some(start), Some(end)) if start >= end) {
            start_at = None;
            end_at = None;
        }
        campaigns.push(Campaign {
            campaign_id: id,
            campaign_key: row.get("campaignKey").and_then(bounded_text),
            claim_status: match row.get("claimStatus").and_then(Value::as_str) {
                Some("CLAIMABLE") => ClaimStatus::Claimable,
                Some("CLAIMED") => ClaimStatus::Claimed,
                _ => ClaimStatus::Unknown,
            },
            start_at,
            end_at,
            benefit_amount: row
                .get("benefit")
                .and_then(|b| b.get("amount"))
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1_000_000_000.0),
        });
    }
    Ok(campaigns)
}
fn bounded_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| {
            !s.trim().is_empty()
                && s.len() <= 256
                && !s.chars().any(|c| {
                    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                })
        })
        .map(str::to_owned)
}
fn timestamp_ms(value: &Value) -> Option<i64> {
    // The pinned upstream listing consumes Unix seconds. Do not guess milliseconds,
    // alternate date formats, a daily round, or eligibility from this interval.
    value
        .as_i64()
        .filter(|n| *n > 0 && *n <= 253_402_300_799)?
        .checked_mul(1000)
}
#[cfg(test)]
#[path = "cn_campaigns_tests.rs"]
mod tests;
