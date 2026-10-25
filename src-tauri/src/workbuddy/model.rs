use super::{checkin::ClaimState, credits::Credits};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Failure {
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
// Credentials are backend-only. Deliberately no Debug implementation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Account {
    pub uid: String,
    pub domain: Domain,
    pub label: String,
    pub token: String,
}
impl Drop for Account {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
    }
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
            account,
            credits: Credits::default(),
            claim_state: ClaimState::Unconfirmed,
            claim_day: String::new(),
            credited: None,
            pending: None,
        }
    }
    pub(super) fn inference_allowed(&self) -> bool {
        self.inference_consent.unwrap_or(self.inference.is_some())
    }
    pub fn credential_binding(&self) -> String {
        let mut hash = Sha256::new();
        for value in [
            self.account.domain.host(),
            &self.account.uid,
            if self.shared_authorization {
                self.inference
                    .as_ref()
                    .map(|grant| grant.id())
                    .unwrap_or("")
            } else {
                &self.account.token
            },
        ] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        hex::encode(hash.finalize())
    }
    pub fn automatic_enabled(&self) -> bool {
        self.auto_claim_enabled
            && self.auto_claim_policy_version == AUTO_CLAIM_POLICY_VERSION
            && self.auto_claim_credential.as_deref() == Some(&self.credential_binding())
    }
    pub fn can_set_automatic(&self) -> bool {
        (if self.shared_authorization {
            self.inference.is_some()
        } else {
            !self.account.token.is_empty()
        }) && self.claim_state != ClaimState::NeedsVerification
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
            can_refresh: true,
            can_claim: self.pending.is_none()
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
