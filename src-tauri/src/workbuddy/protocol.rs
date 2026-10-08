//! WorkBuddy CN transport boundary, adapted from pinned MIT modules.
use super::model::{Account, Failure};
use serde_json::Value;
use std::{future::Future, pin::Pin};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Summary,
    Paid,
    Free,
    Status,
    Claim,
    AuthState,
    AuthToken,
    Profile,
}
impl Endpoint {
    pub fn path(self) -> &'static str {
        match self {
            Self::Summary => "/billing/meter/get-user-resource-summary",
            Self::Paid => "/billing/meter/get-user-resource-paid-packages",
            Self::Free => "/billing/meter/get-user-resource-free-packages",
            Self::Status => "/v2/billing/meter/checkin-activity-status",
            Self::Claim => "/v2/billing/meter/daily-checkin",
            Self::AuthState => "/v2/plugin/auth/state",
            Self::AuthToken => "/v2/plugin/auth/token",
            Self::Profile => "/v2/plugin/login/account",
        }
    }
}
pub(crate) trait Transport: Send + Sync {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        account: Option<&'a Account>,
        state: Option<&'a str>,
        now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>>;
}
