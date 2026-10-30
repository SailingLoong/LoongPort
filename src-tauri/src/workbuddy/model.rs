use super::{
    checkin::ClaimState,
    credits::{CreditExpiryStatus, CreditExpiryView, Credits},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Failure {
    InferencePaused,
    Unconfirmed,
    NeedsVerification,
    UnsupportedContext,
    StorageUnavailable,
    Busy,
    ExpiredAuthorization,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Domain {
    CodeBuddy,
    WorkBuddy,
}
impl Domain {
    pub fn parse(s: &str) -> Result<Self, Failure> {
        match s {
            "www.codebuddy.cn" => Ok(Self::CodeBuddy),
            "www.workbuddy.cn" => Ok(Self::WorkBuddy),
            _ => Err(Failure::UnsupportedContext),
        }
    }
    pub fn host(self) -> &'static str {
        match self {
            Self::CodeBuddy => "www.codebuddy.cn",
            Self::WorkBuddy => "www.workbuddy.cn",
        }
    }
}
// Backend-only credentials, without Debug or DTO serialization.
mod credential;
pub(crate) use credential::Credential;
#[derive(Clone)]
pub(crate) struct Account {
    pub uid: String,
    pub domain: Domain,
    pub label: String,
    pub credential: Credential,
}
impl Account {
    pub fn id(&self, owner: &str) -> String {
        let mut hash = Sha256::new();
        for s in ["workbuddy:cn:v1", owner, self.domain.host(), &self.uid] {
            hash.update((s.len() as u64).to_be_bytes());
            hash.update(s.as_bytes());
        }
        hex::encode(hash.finalize())
    }
}
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attempt {
    pub day: String,
    pub before: Option<f64>,
    pub expected: Option<f64>,
    pub receipt: ClaimState,
}
/// One local reconciliation fact, persisted by the original claim owner.
/// receipt_id is internal correlation only, never a provider receipt or replay permit.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct SuccessfulClaimFact {
    pub confirmed_at_ms: i64,
    pub credited: f64,
    pub unit: Option<String>,
    pub receipt_id: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SuccessfulClaimView {
    pub confirmed_at_ms: Option<i64>,
    pub time_source: Option<&'static str>,
    pub credited: Option<f64>,
    pub unit: Option<String>,
}
impl SuccessfulClaimFact {
    fn view(&self) -> SuccessfulClaimView {
        SuccessfulClaimView {
            confirmed_at_ms: Some(self.confirmed_at_ms),
            time_source: Some("localReconciled"),
            credited: Some(self.credited),
            unit: self.unit.clone(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedAccount {
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub auto_claim_enabled: bool,
    #[serde(default)]
    pub auto_claim_revision: u64,
    #[serde(default)]
    pub auto_claim_policy_version: u32,
    #[serde(default)]
    pub auto_claim_credential: Option<String>,
    #[serde(default)]
    pub attempted_day: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) inference: Option<super::inference::Authorization>,
    // Provenance, not an inferred permission: legacy access-only rows never cross hosts.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) shared_authorization: bool,
    // None preserves an existing explicitly authorized legacy inference grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) inference_consent: Option<bool>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) inference_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) credit_models: Option<super::credits::CreditModelSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last_successful_claim: Option<SuccessfulClaimFact>,
    pub account: Account,
    pub credits: Credits,
    pub claim_state: ClaimState,
    pub claim_day: String,
    pub credited: Option<f64>,
    pub pending: Option<Attempt>,
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
impl SavedAccount {
    pub fn new(account: Account) -> Self {
        Self {
            generation: 0,
            auto_claim_enabled: false,
            auto_claim_revision: 0,
            auto_claim_policy_version: 0,
            auto_claim_credential: None,
            attempted_day: None,
            inference: None,
            shared_authorization: false,
            inference_consent: None,
            inference_revision: 0,
            credit_models: None,
            last_successful_claim: None,
            account,
            credits: Credits::default(),
            claim_state: ClaimState::Unconfirmed,
            claim_day: String::new(),
            credited: None,
            pending: None,
        }
    }
    pub(super) fn inference_allowed(&self) -> bool {
        self.account.is_api() && self.inference_consent.unwrap_or(self.inference.is_some())
    }
    pub fn credential_binding(&self) -> String {
        let mut hash = Sha256::new();
        let api_token = match &self.account.credential {
            Credential::Api(token) => token.as_str(),
            Credential::ApiWithNative { token, snapshot } => {
                hash.update(b"workbuddy:api-native-auto:v1");
                hash.update(credential::fingerprint_with_native(token, snapshot));
                token.as_str()
            }
            Credential::NativeOnly(_) => {
                for value in [
                    "workbuddy:native-only:v1",
                    self.account.domain.host(),
                    &self.account.uid,
                ] {
                    hash.update((value.len() as u64).to_be_bytes());
                    hash.update(value.as_bytes());
                }
                return hex::encode(hash.finalize());
            }
        };
        for value in [
            self.account.domain.host(),
            &self.account.uid,
            if self.shared_authorization {
                self.inference
                    .as_ref()
                    .map(|grant| grant.id())
                    .unwrap_or("")
            } else {
                api_token
            },
        ] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        hex::encode(hash.finalize())
    }
    pub fn automatic_enabled(&self) -> bool {
        self.account.is_api()
            && self.auto_claim_enabled
            && self.auto_claim_policy_version == AUTO_CLAIM_POLICY_VERSION
            && self.auto_claim_credential.as_deref() == Some(&self.credential_binding())
    }
    pub fn can_set_automatic(&self) -> bool {
        self.account.is_api()
            && (if self.shared_authorization {
                self.inference.is_some()
            } else {
                self.account
                    .api_token()
                    .is_ok_and(|token| !token.is_empty())
            })
            && self.claim_state != ClaimState::NeedsVerification
    }
    pub(super) fn validate_for_store(&self) -> Result<(), Failure> {
        self.account.validate_credential()?;
        if matches!(&self.account.credential, Credential::ApiWithNative { token, .. } if token.is_empty())
            && !(self.shared_authorization
                && self
                    .inference
                    .as_ref()
                    .is_some_and(|grant| super::inference::valid_grant(grant, self)))
        {
            return Err(Failure::StorageUnavailable);
        }
        if !self.account.is_api()
            && (self.auto_claim_enabled
                || self.auto_claim_credential.is_some()
                || self.inference.is_some()
                || self.shared_authorization
                || self.inference_consent.is_some()
                || self.pending.is_some()
                || self.attempted_day.is_some())
        {
            return Err(Failure::StorageUnavailable);
        }
        Ok(())
    }
    pub fn view(&self, owner: &str, day: &str) -> AccountView {
        let same_day = self.claim_day == day;
        let state = if same_day {
            self.claim_state
        } else {
            ClaimState::Unconfirmed
        };
        AccountView {
            id: self.account.id(owner),
            label: self.account.label.clone(),
            credits: self.credits.for_display(),
            claim_state: state,
            credited: if same_day { self.credited } else { None },
            last_successful_claim: self
                .last_successful_claim
                .as_ref()
                .map(SuccessfulClaimFact::view),
            credit_expiry_status: CreditExpiryStatus::Unavailable,
            next_known_credit_expiry: None,
            can_refresh: self.account.is_api(),
            can_claim: self.account.is_api()
                && self.pending.is_none()
                && self.attempted_day.as_deref().is_none_or(|old| old < day)
                && matches!(state, ClaimState::Available | ClaimState::Unconfirmed),
            generation: self.generation,
            auto_claim_enabled: self.automatic_enabled(),
            auto_claim_revision: self.auto_claim_revision,
            auto_claim_policy_version: AUTO_CLAIM_POLICY_VERSION,
            can_set_automatic: self.can_set_automatic(),
            auto_claim_blocked: self.pending.is_some(),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountView {
    pub generation: u64,
    pub auto_claim_enabled: bool,
    pub auto_claim_revision: u64,
    pub auto_claim_policy_version: u32,
    pub can_set_automatic: bool,
    pub auto_claim_blocked: bool,
    pub id: String,
    pub label: String,
    pub credits: Credits,
    pub claim_state: ClaimState,
    pub credited: Option<f64>,
    pub can_refresh: bool,
    pub can_claim: bool,
    pub last_successful_claim: Option<SuccessfulClaimView>,
    pub credit_expiry_status: CreditExpiryStatus,
    pub next_known_credit_expiry: Option<CreditExpiryView>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimResult {
    pub account: AccountView,
    pub error: Option<Failure>,
}
pub(crate) fn today(now: i64) -> String {
    use chrono::TimeZone;
    chrono::FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .timestamp_millis_opt(now)
        .single()
        .unwrap()
        .format("%Y-%m-%d")
        .to_string()
}

/// Version of the explicit per-account, running-and-unlocked-only consent.
pub(crate) const AUTO_CLAIM_POLICY_VERSION: u32 = 1;
