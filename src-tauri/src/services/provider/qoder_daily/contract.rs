//! Strict protocol parsing consumed by the explicit Qoder daily claim service.
//! No transport, credential storage, scheduler or device generation.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Region {
    International,
    China,
}

/// These non-secret facts must eventually come from an authenticated account
/// owner. Constructing this test input does not establish that owner exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub provider_id: String,
    pub uid: String,
    pub region: Region,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    UnsupportedRegion,
    IdentityMismatch,
    InvalidResponse,
    TargetMismatch,
    NotClaimable,
    UnsupportedBenefit,
    InvalidWindow,
    AmbiguousTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundCampaign {
    identity: Identity,
    campaign_id: String,
    campaign_key: String,
    start_at: i64,
    end_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub campaign_id: String,
    pub campaign_key: String,
}

/// Caller supplies the current server-observed time, not a local business day.
/// Resolving this time from a trusted response is outside the isolated probe.
pub fn admit(
    identity: &Identity,
    response_region: Region,
    target: &Target,
    body: &Value,
    server_now: i64,
) -> Result<BoundCampaign, Failure> {
    if identity.region != Region::International {
        return Err(Failure::UnsupportedRegion);
    }
    if identity.region != response_region
        || identity.provider_id.trim().is_empty()
        || identity.uid.trim().is_empty()
    {
        return Err(Failure::IdentityMismatch);
    }
    // No URL interpolation is permitted until the selected ID is a single
    // unambiguous path segment. Keys remain opaque, not a guessed daily pattern.
    if !safe_segment(&target.campaign_id) || target.campaign_key.trim().is_empty() {
        return Err(Failure::TargetMismatch);
    }
    if !consistent_identity(body, &identity.uid) {
        return Err(Failure::IdentityMismatch);
    }
    let body = envelope(body, "campaigns").ok_or(Failure::InvalidResponse)?;
    if body.get("uid").and_then(Value::as_str) != Some(identity.uid.as_str()) {
        return Err(Failure::IdentityMismatch);
    }
    let rows = body
        .get("campaigns")
        .and_then(Value::as_array)
        .ok_or(Failure::InvalidResponse)?;
    let mut matches = rows.iter().filter(|row| {
        row.get("campaignId").and_then(Value::as_str) == Some(target.campaign_id.as_str())
    });
    let row = matches.next().ok_or(Failure::TargetMismatch)?;
    if matches.next().is_some() {
        return Err(Failure::AmbiguousTarget);
    }
    if row.get("campaignKey").and_then(Value::as_str) != Some(target.campaign_key.as_str()) {
        return Err(Failure::TargetMismatch);
    }
    if row.get("actionType").and_then(Value::as_str) != Some("CLAIM_BENEFIT")
        || row.get("claimStatus").and_then(Value::as_str) != Some("CLAIMABLE")
    {
        return Err(Failure::NotClaimable);
    }
    if row.pointer("/benefit/kind").and_then(Value::as_str) != Some("CREDITS")
        || row.pointer("/benefit/amount").and_then(Value::as_u64) != Some(100)
    {
        return Err(Failure::UnsupportedBenefit);
    }
    let start_at = row
        .get("startAt")
        .and_then(Value::as_i64)
        .ok_or(Failure::InvalidWindow)?;
    let end_at = row
        .get("endAt")
        .and_then(Value::as_i64)
        .ok_or(Failure::InvalidWindow)?;
    if start_at < 0 || start_at >= end_at || server_now < start_at || server_now >= end_at {
        return Err(Failure::InvalidWindow);
    }
    Ok(BoundCampaign {
        identity: identity.clone(),
        campaign_id: target.campaign_id.clone(),
        campaign_key: target.campaign_key.clone(),
        start_at,
        end_at,
    })
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Never splice identity from one envelope into rows/receipt from another.
fn envelope<'a>(body: &'a Value, discriminator: &str) -> Option<&'a Value> {
    if !non_error_envelope(body)
        || body
            .get("data")
            .is_some_and(|data| !non_error_envelope(data))
    {
        return None;
    }
    match (body.get(discriminator), body.get("data")) {
        (Some(_), Some(data)) if data.get(discriminator).is_some() => None,
        (Some(_), _) => Some(body),
        (None, Some(data)) if data.is_object() => Some(data),
        _ => Some(body),
    }
}

pub(super) fn non_error_envelope(body: &Value) -> bool {
    body.is_object()
        && body
            .get("success")
            .is_none_or(|v| v.as_bool() == Some(true))
        && body
            .get("code")
            .is_none_or(|v| matches!(v.as_i64(), Some(0 | 200)))
        && body
            .get("errorCode")
            .is_none_or(|v| v.is_null() || v.as_str() == Some("") || v.as_i64() == Some(0))
        && body.get("error").is_none_or(Value::is_null)
}

fn consistent_identity(body: &Value, expected_uid: &str) -> bool {
    [Some(body), body.get("data")]
        .into_iter()
        .flatten()
        .all(|envelope| {
            envelope
                .get("uid")
                .is_none_or(|uid| uid.as_str() == Some(expected_uid))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    NewlyClaimed,
    AlreadyClaimed,
    ClaimedDeltaUnknown,
    #[serde(rename = "OutcomeUnknown")]
    Unknown,
    Conflict,
    VerificationRequired,
    RateLimited,
}
impl Outcome {
    pub fn added_credits(self) -> Option<u16> {
        match self {
            Self::NewlyClaimed => Some(100),
            Self::AlreadyClaimed => Some(0),
            _ => None,
        }
    }
}

pub fn receipt(
    binding: &BoundCampaign,
    response_identity: &Identity,
    http_status: u16,
    body: Option<&Value>,
) -> Outcome {
    if response_identity != &binding.identity {
        return Outcome::Unknown;
    }
    match http_status {
        401 | 403 => return Outcome::VerificationRequired,
        429 => return Outcome::RateLimited,
        200 | 409 => {}
        _ => return Outcome::Unknown,
    }
    let fallback = if http_status == 409 {
        Outcome::Conflict
    } else {
        Outcome::Unknown
    };
    let Some(body) = body
        .filter(|v| consistent_identity(v, &binding.identity.uid))
        .and_then(|v| envelope(v, "status"))
    else {
        return fallback;
    };
    if body.get("uid").is_some()
        && body.get("uid").and_then(Value::as_str) != Some(binding.identity.uid.as_str())
    {
        return fallback;
    }
    if body.get("status").and_then(Value::as_str) != Some("CLAIMED")
        || body.get("campaignId").and_then(Value::as_str) != Some(binding.campaign_id.as_str())
        || body.get("campaignKey").and_then(Value::as_str) != Some(binding.campaign_key.as_str())
        || body
            .get("grantId")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .is_none()
    {
        return fallback;
    }
    let claimed_at = body
        .get("claimedAt")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp());
    if !matches!(claimed_at, Some(time) if time >= binding.start_at && time < binding.end_at) {
        return fallback;
    }
    match body.get("replayed") {
        Some(Value::Bool(true)) => Outcome::AlreadyClaimed,
        Some(Value::Bool(false)) if http_status == 200 => Outcome::NewlyClaimed,
        None if http_status == 200 => Outcome::ClaimedDeltaUnknown,
        _ => fallback,
    }
}

/// A caller-owned persisted attempt; never stored by this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub binding: BoundCampaign,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GetRewardRequest {
    pub origin: &'static str,
    pub path: String,
}

/// There is deliberately no POST variant in a recovery request.
pub fn reconcile_request(
    pending: &Pending,
    identity: &Identity,
) -> Result<GetRewardRequest, Failure> {
    if identity != &pending.binding.identity {
        return Err(Failure::IdentityMismatch);
    }
    if identity.region != Region::International {
        return Err(Failure::UnsupportedRegion);
    }
    Ok(GetRewardRequest {
        origin: "https://openapi.qoder.sh",
        path: format!(
            "/sash/api/v1/me/campaigns/{}/reward",
            pending.binding.campaign_id
        ),
    })
}

pub fn reconcile(
    pending: &Pending,
    identity: &Identity,
    http_status: u16,
    body: Option<&Value>,
) -> Outcome {
    // A read proves grant existence, not which attempt caused the new credits.
    match receipt(&pending.binding, identity, http_status, body) {
        Outcome::NewlyClaimed | Outcome::AlreadyClaimed => Outcome::ClaimedDeltaUnknown,
        other => other,
    }
}

impl BoundCampaign {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn target(&self) -> Target {
        Target {
            campaign_id: self.campaign_id.clone(),
            campaign_key: self.campaign_key.clone(),
        }
    }
    pub fn end_at(&self) -> i64 {
        self.end_at
    }
    pub fn valid_persisted(&self) -> bool {
        self.identity.region == Region::International
            && !self.identity.uid.trim().is_empty()
            && !self.identity.provider_id.trim().is_empty()
            && safe_segment(&self.campaign_id)
            && !self.campaign_key.trim().is_empty()
            && self.start_at >= 0
            && self.start_at < self.end_at
    }
}
impl BoundCampaign {
    pub fn start_at(&self) -> i64 {
        self.start_at
    }
    pub(crate) fn from_operation(
        identity: Identity,
        campaign_id: String,
        campaign_key: String,
        start_at: i64,
        end_at: i64,
    ) -> Option<Self> {
        let bound = Self {
            identity,
            campaign_id,
            campaign_key,
            start_at,
            end_at,
        };
        bound.valid_persisted().then_some(bound)
    }
}
pub(crate) fn listing_identity_matches(body: &Value, identity: &Identity) -> bool {
    consistent_identity(body, &identity.uid)
        && envelope(body, "campaigns")
            .and_then(|v| v.get("uid"))
            .and_then(Value::as_str)
            == Some(identity.uid.as_str())
}

/// A previously observed operation never adopts changed campaign boundaries.
/// CLAIMED listings may omit benefit/action, so compare identity and binding only.
pub(crate) fn listing_matches_binding(body: &Value, binding: &BoundCampaign) -> bool {
    if !consistent_identity(body, &binding.identity.uid) {
        return false;
    }
    let Some(body) = envelope(body, "campaigns") else {
        return false;
    };
    if body.get("uid").and_then(Value::as_str) != Some(binding.identity.uid.as_str()) {
        return false;
    }
    let Some(rows) = body.get("campaigns").and_then(Value::as_array) else {
        return false;
    };
    let mut rows = rows.iter().filter(|r| {
        r.get("campaignId").and_then(Value::as_str) == Some(binding.campaign_id.as_str())
    });
    let Some(row) = rows.next() else {
        return false;
    };
    rows.next().is_none()
        && row.get("campaignKey").and_then(Value::as_str) == Some(binding.campaign_key.as_str())
        && row.get("startAt").and_then(Value::as_i64) == Some(binding.start_at)
        && row.get("endAt").and_then(Value::as_i64) == Some(binding.end_at)
}

/// Strict bootstrap subject from the same authenticated listing envelope.
pub(crate) fn listing_subject(body: &Value) -> Option<&str> {
    let selected = envelope(body, "campaigns")?;
    selected.get("campaigns")?.as_array()?;
    let uid = selected.get("uid")?.as_str()?;
    (!uid.trim().is_empty() && uid.len() <= 512 && consistent_identity(body, uid)).then_some(uid)
}

/// The UI supplies no guessed daily key. Ambiguous or unsupported listings do
/// not choose the first row: exactly one fully admissible target is required.
pub(crate) fn unique_daily_target(
    identity: &Identity,
    body: &Value,
    now: i64,
) -> Result<Option<Target>, Failure> {
    let rows = envelope(body, "campaigns")
        .and_then(|v| v.get("campaigns"))
        .and_then(Value::as_array)
        .ok_or(Failure::InvalidResponse)?;
    let mut targets = rows.iter().filter_map(|row| {
        let target = Target {
            campaign_id: row.get("campaignId")?.as_str()?.to_owned(),
            campaign_key: row.get("campaignKey")?.as_str()?.to_owned(),
        };
        admit(identity, identity.region, &target, body, now)
            .ok()
            .map(|_| target)
    });
    let first = targets.next();
    if targets.next().is_some() {
        return Err(Failure::AmbiguousTarget);
    }
    Ok(first)
}
