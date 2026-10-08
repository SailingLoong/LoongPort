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
#[derive(Clone, Default, Serialize, Deserialize)]
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
    pub account: Account,
    pub credits: Credits,
    pub claim_state: ClaimState,
    pub claim_day: String,
    pub credited: Option<f64>,
    pub pending: Option<Attempt>,
}
impl SavedAccount {
    pub fn new(account: Account) -> Self {
        Self {
            account,
            credits: Credits::default(),
            claim_state: ClaimState::Unconfirmed,
            claim_day: String::new(),
            credited: None,
            pending: None,
        }
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
            credits: self.credits.clone(),
            claim_state: state,
            credited: if same_day { self.credited } else { None },
            can_refresh: true,
            can_claim: matches!(state, ClaimState::Available | ClaimState::Unconfirmed),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountView {
    pub id: String,
    pub label: String,
    pub credits: Credits,
    pub claim_state: ClaimState,
    pub credited: Option<f64>,
    pub can_refresh: bool,
    pub can_claim: bool,
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
