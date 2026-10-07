//! Independent read-only evidence for one complete selected session.
//! Protocol: zai-org/ZCode 29628c9acdb81b703bbd4080c207a0e7ce5e276e,
//! codingPlanProviderAvailability, bigmodelStartPlanZcodeJwt, codingPlanEntitlement,
//! zaiStartPlanBilling and bigmodelUsageQuotaMapper. No identity-based authorization.
use super::core::{AccountSnapshot, OAuthFamily};
use super::native::NativeCipher;
use super::official::{
    BusinessToken, CodingKey, OfficialClient, OfficialError, OfficialTransport, RequestCheck,
    StartJwt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CheckState {
    Accepted,
    Unavailable,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CheckReason {
    Unverified,
    MissingCredential,
    InvalidCredential,
    AppVersionUnknown,
    AuthRejected,
    BusinessRejected,
    MalformedResponse,
    Timeout,
    Network,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CredentialSource {
    BusinessToken,
    GlobalStartJwt,
    AccountStartJwt,
    CodingKey,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CheckResult {
    pub state: CheckState,
    pub reason: Option<CheckReason>,
    pub checked_at: Option<u64>,
    pub source: Option<CredentialSource>,
    pub latest_failure: Option<CheckFailure>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CheckFailure {
    pub reason: CheckReason,
    pub checked_at: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum EntitlementState {
    Available,
    Unavailable,
    Pending,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BusinessCheck {
    pub check: CheckResult,
    pub official_owner_id: Option<String>,
    pub display_name: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartEntitlement {
    pub entitlement_id: Option<String>,
    pub show_name: Option<String>,
    pub period: Option<String>,
    pub effective_at_seconds: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartPlan {
    pub user_plan_id: Option<String>,
    pub plan_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    pub starts_at_seconds: Option<f64>,
    pub ends_at_seconds: Option<f64>,
    pub entitlements: Vec<StartEntitlement>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartBucket {
    pub bucket_id: Option<String>,
    pub user_plan_id: Option<String>,
    pub plan_id: Option<String>,
    pub entitlement_id: Option<String>,
    pub show_name: Option<String>,
    pub meter: Option<String>,
    pub unit_type: Option<String>,
    pub capabilities: Vec<String>,
    pub total_units: Option<f64>,
    pub used_units: Option<f64>,
    pub reserved_units: Option<f64>,
    pub remaining_units: Option<f64>,
    pub available_units: Option<f64>,
    pub period_start_seconds: Option<f64>,
    pub period_end_seconds: Option<f64>,
    pub expires_at_seconds: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartCheck {
    pub check: CheckResult,
    pub entitlement: EntitlementState,
    pub effective_at_seconds: Option<f64>,
    pub quota: CheckResult,
    pub server_time_seconds: Option<f64>,
    pub plans: Vec<StartPlan>,
    pub buckets: Vec<StartBucket>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodingSubscription {
    pub product_id: Option<String>,
    pub product_name: Option<String>,
    pub status: String,
    pub in_current_period: bool,
    pub billing_cycle: Option<String>,
    pub next_renew_time: Option<String>,
    pub valid: Option<String>,
    pub auto_renew: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodingUsageDetail {
    pub model_code: Option<String>,
    pub display_name: Option<String>,
    pub usage: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodingLimit {
    pub limit_type: String,
    pub unit: Option<f64>,
    pub number: Option<f64>,
    pub usage: Option<f64>,
    pub current_value: Option<f64>,
    pub remaining: Option<f64>,
    pub percentage: Option<f64>,
    pub next_reset_time_ms: Option<f64>,
    pub usage_details: Vec<CodingUsageDetail>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CodingCheck {
    pub check: CheckResult,
    pub subscription: CheckResult,
    pub entitlement: EntitlementState,
    pub quota: CheckResult,
    pub subscriptions: Vec<CodingSubscription>,
    pub limits: Vec<CodingLimit>,
}
/// The only command-safe projection. No credentials, hashes or raw responses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionCheckDisplay {
    pub selected_profile_id: String,
    pub business: BusinessCheck,
    pub start: StartCheck,
    pub coding: CodingCheck,
}
/// Backend-only encrypted-catalog record. Never return this type from a command.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionCheckReport {
    display: SessionCheckDisplay,
    bindings: CredentialBindings,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialBindings {
    family: String,
    business: Option<[u8; 32]>,
    start: Option<[u8; 32]>,
    start_app_version: Option<String>,
    coding: Option<[u8; 32]>,
}
impl SessionCheckReport {
    pub(crate) fn display(&self) -> SessionCheckDisplay {
        self.display.clone()
    }
    /// Start balance acceptance is specific to the actual native client's policy version.
    pub(crate) fn start_checked_for(&self, app_version: &str) -> bool {
        self.bindings.start_app_version.as_deref() == Some(app_version)
            && self.display.start.check.state == CheckState::Accepted
            && self.display.start.check.checked_at.is_some()
    }
    /// A transient refresh cannot disprove prior same-credential acceptance.
    /// Keep its original time and expose the new failure; never reuse old quota.
    pub(crate) fn preserve_previous_acceptance(
        &self,
        previous: &Self,
        native: &NativeCipher,
        snapshot: &AccountSnapshot,
    ) -> Self {
        let current = Credentials::load(native, snapshot).bindings(snapshot);
        let mut merged = self.clone();
        if self.bindings.family != current.family || previous.bindings.family != current.family {
            return merged;
        }
        let matches = |new: Option<[u8; 32]>, old: Option<[u8; 32]>, actual: Option<[u8; 32]>| {
            actual.is_some() && actual == new && actual == old
        };
        if matches(
            self.bindings.business,
            previous.bindings.business,
            current.business,
        ) {
            if let Some(check) = preserve_check(
                &self.display.business.check,
                &previous.display.business.check,
            ) {
                merged.display.business = previous.display.business.clone();
                merged.display.business.check = check;
            }
        }
        if matches(self.bindings.start, previous.bindings.start, current.start)
            && self.bindings.start_app_version.is_some()
            && self.bindings.start_app_version == previous.bindings.start_app_version
        {
            let was_entitled = matches!(
                previous.display.start.entitlement,
                EntitlementState::Available | EntitlementState::Pending
            );
            let prior_active: Vec<_> = previous
                .display
                .start
                .plans
                .iter()
                .filter(|plan| plan.status.as_deref() == Some("active"))
                .collect();
            let expired = was_entitled
                && self.display.start.check.checked_at.is_some_and(|now| {
                    !prior_active.is_empty()
                        && prior_active.iter().all(|plan| {
                            plan.ends_at_seconds
                                .is_some_and(|end| end > 0.0 && end <= now as f64)
                        })
                });
            if !expired {
                if let Some(check) =
                    preserve_check(&self.display.start.check, &previous.display.start.check)
                {
                    merged.display.start.check = check;
                    merged.display.start.entitlement = previous.display.start.entitlement;
                    merged.display.start.effective_at_seconds =
                        previous.display.start.effective_at_seconds;
                    merged.display.start.plans = previous.display.start.plans.clone();
                    merged.display.start.server_time_seconds =
                        previous.display.start.server_time_seconds;
                }
            }
        }
        if matches(
            self.bindings.coding,
            previous.bindings.coding,
            current.coding,
        ) && self.display.coding.subscription.reason != Some(CheckReason::AuthRejected)
            && self.display.coding.quota.reason != Some(CheckReason::AuthRejected)
        {
            if let Some(check) =
                preserve_check(&self.display.coding.check, &previous.display.coding.check)
            {
                merged.display.coding.check = check;
            }
            if let Some(check) = preserve_check(
                &self.display.coding.subscription,
                &previous.display.coding.subscription,
            ) {
                merged.display.coding.subscription = check;
                merged.display.coding.entitlement = previous.display.coding.entitlement;
                merged.display.coding.subscriptions = previous.display.coding.subscriptions.clone();
            }
        }
        merged
    }
    pub(crate) fn matches_business(
        &self,
        native: &NativeCipher,
        snapshot: &AccountSnapshot,
    ) -> bool {
        let current = Credentials::load(native, snapshot).bindings(snapshot);
        self.bindings.family == current.family && self.bindings.business == current.business
    }
    pub(crate) fn matches_start(&self, native: &NativeCipher, snapshot: &AccountSnapshot) -> bool {
        let current = Credentials::load(native, snapshot).bindings(snapshot);
        self.bindings.family == current.family && self.bindings.start == current.start
    }
    pub(crate) fn matches_coding(&self, native: &NativeCipher, snapshot: &AccountSnapshot) -> bool {
        let current = Credentials::load(native, snapshot).bindings(snapshot);
        self.bindings.family == current.family && self.bindings.coding == current.coding
    }
    /// Re-encryption does not revoke evidence. Replacement of one consumed secret
    /// clears only that capability, including any business-owner claim it supported.
    pub(crate) fn retain_matching(
        &self,
        native: &NativeCipher,
        snapshot: &AccountSnapshot,
    ) -> Self {
        let credentials = Credentials::load(native, snapshot);
        let mut current = Self::unverified(snapshot, &credentials);
        if self.bindings.family != current.bindings.family {
            return current;
        }
        if self.bindings.business == current.bindings.business {
            current.display.business = self.display.business.clone();
        }
        if self.bindings.start == current.bindings.start {
            current.display.start = self.display.start.clone();
            current.bindings.start_app_version = self.bindings.start_app_version.clone();
            current.display.start.check.source = credentials.source(credentials.start_index);
            current.display.start.quota.source = credentials.source(credentials.start_index);
        }
        if self.bindings.coding == current.bindings.coding {
            current.display.coding = self.display.coding.clone();
        }
        current
    }
    fn unverified(snapshot: &AccountSnapshot, credentials: &Credentials) -> Self {
        let business = credentials.initial(1);
        let start = credentials.initial(credentials.start_index);
        let coding = credentials.initial(5);
        Self {
            bindings: credentials.bindings(snapshot),
            display: SessionCheckDisplay {
                selected_profile_id: snapshot.identity().opaque_id(),
                business: BusinessCheck {
                    check: business,
                    official_owner_id: None,
                    display_name: None,
                },
                start: StartCheck {
                    check: start.clone(),
                    entitlement: EntitlementState::Unknown,
                    effective_at_seconds: None,
                    quota: start,
                    server_time_seconds: None,
                    plans: Vec::new(),
                    buckets: Vec::new(),
                },
                coding: CodingCheck {
                    check: coding.clone(),
                    subscription: coding.clone(),
                    quota: coding,
                    entitlement: EntitlementState::Unknown,
                    subscriptions: Vec::new(),
                    limits: Vec::new(),
                },
            },
        }
    }
}
pub(crate) async fn check_session<T: OfficialTransport>(
    client: &OfficialClient<T>,
    native: &NativeCipher,
    snapshot: &AccountSnapshot,
    app_version: Option<&str>,
    checked_at: u64,
    check: &RequestCheck<'_>,
) -> Result<SessionCheckReport, OfficialError> {
    check()?;
    let credentials = Credentials::load(native, snapshot);
    let mut report = SessionCheckReport::unverified(snapshot, &credentials);
    let family = snapshot.identity().family();
    if let Some(secret) = credentials.secret(1) {
        let result = match BusinessToken::from_stored(family, secret) {
            Ok(token) => client.read_customer(&token, check).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(customer) => {
                report.display.business.check = accepted(credentials.source(1), checked_at);
                report.display.business.official_owner_id =
                    credentials.safe_text(customer.identity_id.as_deref());
                report.display.business.display_name =
                    credentials.safe_text(customer.display_name.as_deref());
            }
            Err(error) => {
                report.display.business.check = failed(error, credentials.source(1), checked_at)?
            }
        }
    }
    let source = credentials.source(credentials.start_index);
    if let Some(secret) = credentials.secret(credentials.start_index) {
        if let Some(version) = app_version.filter(|version| !version.trim().is_empty()) {
            // Keep the real policy input with accepted/failed Start evidence.
            // It remains backend-only and is never substituted with our app version.
            report.bindings.start_app_version = Some(version.to_owned());
            let result = match StartJwt::new(secret) {
                Ok(jwt) => client.read_start_balance(&jwt, version, check).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(data) => {
                    report.display.start =
                        parse_start(data.value(), &credentials, source, checked_at)
                }
                Err(error) => {
                    let result = failed(error, source, checked_at)?;
                    report.display.start.check = result.clone();
                    report.display.start.quota = result;
                }
            }
        } else {
            let result = unknown(CheckReason::AppVersionUnknown, source, None);
            report.display.start.check = result.clone();
            report.display.start.quota = result;
        }
    }
    let source = credentials.source(5);
    if let Some(secret) = credentials.secret(5) {
        match CodingKey::new(secret) {
            Ok(key) => {
                match client.read_coding_subscription(family, &key, check).await {
                    Ok(data) => {
                        let (result, entitlement, subscriptions) =
                            parse_subscriptions(data.value(), &credentials, source, checked_at);
                        report.display.coding.subscription = result;
                        report.display.coding.entitlement = entitlement;
                        report.display.coding.subscriptions = subscriptions;
                    }
                    Err(error) => {
                        report.display.coding.subscription = failed(error, source, checked_at)?
                    }
                }
                match client.read_coding_quota(family, &key, check).await {
                    Ok(data) => {
                        let (result, limits) =
                            parse_coding_quota(data.value(), &credentials, source, checked_at);
                        report.display.coding.quota = result;
                        report.display.coding.limits = limits;
                    }
                    Err(error) => report.display.coding.quota = failed(error, source, checked_at)?,
                }
                // Either endpoint can accept the key; quota failure never erases
                // independently established subscription entitlement.
                report.display.coding.check = [
                    &report.display.coding.subscription,
                    &report.display.coding.quota,
                ]
                .into_iter()
                .find(|result| result.state == CheckState::Accepted)
                .unwrap_or(&report.display.coding.subscription)
                .clone();
            }
            Err(error) => {
                let result = failed(error, source, checked_at)?;
                report.display.coding.check = result.clone();
                report.display.coding.subscription = result.clone();
                report.display.coding.quota = result;
            }
        }
    }
    check()?;
    Ok(report)
}

struct Credentials {
    values: Vec<Option<Zeroizing<String>>>,
    present: Vec<bool>,
    start_index: usize,
}
impl Credentials {
    fn load(native: &NativeCipher, snapshot: &AccountSnapshot) -> Self {
        let document = snapshot.scoped_document();
        let keys = snapshot.identity().credential_keys();
        let present: Vec<_> = keys.iter().map(|key| document.get(key).is_some()).collect();
        // Official availability consumes global JWT first for either selected family.
        // A present but damaged/rejected global JWT is never silently replaced.
        let start_index = if present[4] { 4 } else { 6 };
        let values = keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                if !matches!(index, 1 | 2 | 4 | 5 | 6) {
                    return None;
                }
                document
                    .get(key)
                    .and_then(|raw| native.decrypt(raw).ok())
                    .map(|value| Zeroizing::new(value.trim().to_owned()))
                    .filter(|value| !value.is_empty())
            })
            .collect();
        Self {
            values,
            present,
            start_index,
        }
    }
    fn secret(&self, index: usize) -> Option<&str> {
        self.values[index].as_deref().map(String::as_str)
    }
    fn source(&self, index: usize) -> Option<CredentialSource> {
        self.present[index].then_some(match index {
            1 => CredentialSource::BusinessToken,
            4 => CredentialSource::GlobalStartJwt,
            5 => CredentialSource::CodingKey,
            _ => CredentialSource::AccountStartJwt,
        })
    }
    fn initial(&self, index: usize) -> CheckResult {
        if !self.present[index] {
            CheckResult {
                state: CheckState::Unavailable,
                reason: Some(CheckReason::MissingCredential),
                checked_at: None,
                source: None,
                latest_failure: None,
            }
        } else {
            unknown(
                if self.secret(index).is_some() {
                    CheckReason::Unverified
                } else {
                    CheckReason::InvalidCredential
                },
                self.source(index),
                None,
            )
        }
    }
    fn bindings(&self, snapshot: &AccountSnapshot) -> CredentialBindings {
        let digest = |index| {
            self.secret(index)
                .map(|secret| Sha256::digest(secret.as_bytes()).into())
        };
        CredentialBindings {
            family: match snapshot.identity().family() {
                OAuthFamily::Zai => "zai",
                OAuthFamily::BigModel => "bigmodel",
            }
            .into(),
            business: digest(1),
            start: digest(self.start_index),
            start_app_version: None,
            coding: digest(5),
        }
    }
    fn safe_text(&self, value: Option<&str>) -> Option<String> {
        let value = value?.trim();
        if value.is_empty()
            || value.chars().count() > 160
            || value.chars().any(char::is_control)
            || self
                .values
                .iter()
                .flatten()
                .any(|secret| value.contains(secret.as_str()))
        {
            return None;
        }
        Some(value.into())
    }
    fn text(&self, value: &Value, field: &str) -> Option<String> {
        self.safe_text(value.get(field).and_then(Value::as_str))
    }
}
fn accepted(source: Option<CredentialSource>, checked_at: u64) -> CheckResult {
    CheckResult {
        state: CheckState::Accepted,
        reason: None,
        checked_at: Some(checked_at),
        source,
        latest_failure: None,
    }
}
fn preserve_check(current: &CheckResult, previous: &CheckResult) -> Option<CheckResult> {
    if current.state != CheckState::Unknown
        || previous.state != CheckState::Accepted
        || previous.checked_at.is_none()
    {
        return None;
    }
    let reason = current.reason?;
    if !matches!(reason, CheckReason::Timeout | CheckReason::Network) {
        return None;
    }
    let mut retained = previous.clone();
    retained.source = current.source;
    retained.latest_failure = Some(CheckFailure {
        reason,
        checked_at: current.checked_at?,
    });
    Some(retained)
}
fn unknown(
    reason: CheckReason,
    source: Option<CredentialSource>,
    checked_at: Option<u64>,
) -> CheckResult {
    CheckResult {
        state: CheckState::Unknown,
        reason: Some(reason),
        checked_at,
        source,
        latest_failure: None,
    }
}
fn malformed(source: Option<CredentialSource>, checked_at: u64) -> CheckResult {
    unknown(CheckReason::MalformedResponse, source, Some(checked_at))
}
fn failed(
    error: OfficialError,
    source: Option<CredentialSource>,
    checked_at: u64,
) -> Result<CheckResult, OfficialError> {
    let (state, reason) = match error {
        OfficialError::Cancelled => return Err(error),
        OfficialError::Unauthorized => (CheckState::Unavailable, CheckReason::AuthRejected),
        OfficialError::BusinessRejected => (CheckState::Unavailable, CheckReason::BusinessRejected),
        OfficialError::InvalidInput => (CheckState::Unknown, CheckReason::InvalidCredential),
        OfficialError::InvalidResponse | OfficialError::ResponseTooLarge => {
            (CheckState::Unknown, CheckReason::MalformedResponse)
        }
        OfficialError::Timeout => (CheckState::Unknown, CheckReason::Timeout),
        _ => (CheckState::Unknown, CheckReason::Network),
    };
    Ok(CheckResult {
        state,
        reason: Some(reason),
        checked_at: Some(checked_at),
        source,
        latest_failure: None,
    })
}
fn number(value: &Value, field: &str, allow_string: bool) -> Option<f64> {
    let value = value.get(field)?;
    let number = value.as_f64().or_else(|| {
        allow_string
            .then(|| value.as_str()?.trim().parse::<f64>().ok())
            .flatten()
    })?;
    (number.is_finite() && number >= 0.0).then_some(number)
}
fn numeric_fields_valid(value: &Value, fields: &[&str], allow_string: bool) -> bool {
    fields.iter().all(|field| {
        value.get(*field).is_none_or(Value::is_null) || number(value, field, allow_string).is_some()
    })
}
fn parse_start(
    value: &Value,
    credentials: &Credentials,
    source: Option<CredentialSource>,
    checked_at: u64,
) -> StartCheck {
    let mut start = StartCheck {
        check: malformed(source, checked_at),
        entitlement: EntitlementState::Unknown,
        effective_at_seconds: None,
        quota: malformed(source, checked_at),
        server_time_seconds: number(value, "server_time", false),
        plans: Vec::new(),
        buckets: Vec::new(),
    };
    if let Some(plans) = value.get("plans").and_then(Value::as_array) {
        let mut malformed_plan = false;
        for value in plans {
            if !numeric_fields_valid(value, &["starts_at", "ends_at"], true)
                || ["plan_id", "user_plan_id", "name"].iter().any(|field| {
                    value
                        .get(*field)
                        .is_some_and(|value| !value.is_null() && !value.is_string())
                })
                || value.get("entitlements").is_some_and(|value| {
                    !value.is_null()
                        && !value.as_array().is_some_and(|entries| {
                            entries.iter().all(|entry| {
                                entry.is_object()
                                    && numeric_fields_valid(entry, &["effective_at"], true)
                            })
                        })
                })
            {
                malformed_plan = true;
                continue;
            }
            let Some(status) = credentials.text(value, "status") else {
                malformed_plan = true;
                continue;
            };
            let mut status = status.to_lowercase();
            let ends_at = number(value, "ends_at", true);
            if status == "active"
                && ends_at.is_some_and(|end| end > 0.0 && end <= checked_at as f64)
            {
                status = "expired".into();
            }
            let entitlements = value
                .get("entitlements")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter(|value| value.is_object())
                        .map(|entry| StartEntitlement {
                            entitlement_id: credentials.text(entry, "entitlement_id"),
                            show_name: credentials.text(entry, "show_name"),
                            period: credentials.text(entry, "period"),
                            effective_at_seconds: number(entry, "effective_at", true),
                        })
                        .collect()
                })
                .unwrap_or_default();
            start.plans.push(StartPlan {
                user_plan_id: credentials.text(value, "user_plan_id"),
                plan_id: credentials.text(value, "plan_id"),
                name: credentials.text(value, "name"),
                status: Some(status),
                starts_at_seconds: number(value, "starts_at", true),
                ends_at_seconds: ends_at,
                entitlements,
            });
        }
        let active: Vec<_> = start
            .plans
            .iter()
            .filter(|plan| {
                let matches = plan.plan_id.is_none() && plan.name.is_none()
                    || [plan.plan_id.as_deref(), plan.name.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|value| {
                            let value = value.to_lowercase();
                            value.contains("start-plan") || value.contains("start plan")
                        });
                matches && plan.status.as_deref() == Some("active")
            })
            .collect();
        if !active.is_empty() || !malformed_plan {
            start.check = accepted(source, checked_at);
            start.entitlement = if active.is_empty() {
                EntitlementState::Unavailable
            } else {
                EntitlementState::Available
            };
            let times: Vec<_> = active
                .iter()
                .flat_map(|plan| {
                    if plan.entitlements.is_empty() {
                        vec![plan.starts_at_seconds]
                    } else {
                        plan.entitlements
                            .iter()
                            .map(|entry| entry.effective_at_seconds)
                            .collect()
                    }
                })
                .collect();
            let now = start.server_time_seconds.unwrap_or(checked_at as f64);
            let has_model =
                value
                    .get("balances")
                    .and_then(Value::as_array)
                    .is_some_and(|balances| {
                        balances.iter().any(|bucket| {
                            !expired_bucket(bucket, &start.plans, credentials)
                                && bucket_has_model(bucket)
                        })
                    });
            if !has_model
                && !times.is_empty()
                && times.iter().all(|time| time.is_some_and(|time| time > now))
            {
                start.entitlement = EntitlementState::Pending;
                start.effective_at_seconds = times.into_iter().flatten().min_by(f64::total_cmp);
            }
        }
    }
    if let Some(balances) = value.get("balances").and_then(Value::as_array) {
        let mut malformed_bucket = false;
        for value in balances {
            if expired_bucket(value, &start.plans, credentials) {
                continue;
            }
            if !value.is_object()
                || !numeric_fields_valid(
                    value,
                    &[
                        "total_units",
                        "used_units",
                        "reserved_units",
                        "remaining_units",
                        "available_units",
                        "period_start",
                        "period_end",
                        "expires_at",
                    ],
                    true,
                )
            {
                malformed_bucket = true;
                continue;
            }
            let capabilities = value
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| credentials.safe_text(value.as_str()))
                        .collect()
                })
                .unwrap_or_default();
            start.buckets.push(StartBucket {
                bucket_id: credentials.text(value, "bucket_id"),
                user_plan_id: credentials.text(value, "user_plan_id"),
                plan_id: credentials.text(value, "plan_id"),
                entitlement_id: credentials.text(value, "entitlement_id"),
                show_name: credentials.text(value, "show_name"),
                meter: credentials.text(value, "meter"),
                unit_type: credentials.text(value, "unit_type"),
                capabilities,
                total_units: number(value, "total_units", true),
                used_units: number(value, "used_units", true),
                reserved_units: number(value, "reserved_units", true),
                remaining_units: number(value, "remaining_units", true),
                available_units: number(value, "available_units", true),
                period_start_seconds: number(value, "period_start", true),
                period_end_seconds: number(value, "period_end", true),
                expires_at_seconds: number(value, "expires_at", true),
            });
        }
        if !malformed_bucket {
            start.quota = accepted(source, checked_at);
        }
    }
    start
}
fn bucket_has_model(value: &Value) -> bool {
    value
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|caps| {
            caps.iter().filter_map(Value::as_str).any(|capability| {
                let capability = capability.trim();
                capability.to_lowercase().starts_with("model:")
                    && !capability[6..].trim().is_empty()
            })
        })
        || value
            .get("show_name")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty())
}
fn expired_bucket(value: &Value, plans: &[StartPlan], credentials: &Credentials) -> bool {
    let user_plan_id = credentials.text(value, "user_plan_id");
    let plan_id = credentials.text(value, "plan_id");
    let mut owners = plans
        .iter()
        .filter(|plan| match (&user_plan_id, &plan.user_plan_id) {
            (Some(bucket), Some(owner)) => bucket == owner,
            _ => {
                matches!((&plan_id, &plan.plan_id), (Some(bucket), Some(owner)) if bucket == owner)
            }
        })
        .peekable();
    owners.peek().is_some() && owners.all(|plan| plan.status.as_deref() == Some("expired"))
}
fn parse_subscriptions(
    value: &Value,
    credentials: &Credentials,
    source: Option<CredentialSource>,
    checked_at: u64,
) -> (CheckResult, EntitlementState, Vec<CodingSubscription>) {
    let Some(list) = value.as_array() else {
        return (
            malformed(source, checked_at),
            EntitlementState::Unknown,
            Vec::new(),
        );
    };
    let mut subscriptions = Vec::new();
    let mut malformed_coding = false;
    for value in list {
        let is_coding = ["productId", "productName"].iter().any(|field| {
            value
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|value| value.to_lowercase().contains("coding"))
        });
        if !is_coding {
            continue;
        }
        let valid_optional = [
            "productId",
            "productName",
            "nextRenewTime",
            "valid",
            "billingCycle",
        ]
        .iter()
        .all(|field| value.get(*field).is_none_or(Value::is_string));
        let renew_valid = value
            .get("autoRenew")
            .is_none_or(|value| value.is_boolean() || value.is_number());
        let (Some(status), Some(in_current_period)) = (
            credentials.text(value, "status"),
            value.get("inCurrentPeriod").and_then(Value::as_bool),
        ) else {
            malformed_coding = true;
            continue;
        };
        if !valid_optional || !renew_valid {
            malformed_coding = true;
            continue;
        }
        subscriptions.push(CodingSubscription {
            product_id: credentials.text(value, "productId"),
            product_name: credentials.text(value, "productName"),
            status,
            in_current_period,
            billing_cycle: credentials.text(value, "billingCycle"),
            next_renew_time: credentials.text(value, "nextRenewTime"),
            valid: credentials.text(value, "valid"),
            auto_renew: value.get("autoRenew").and_then(|value| {
                value
                    .as_bool()
                    .or_else(|| value.as_f64().map(|number| number == 1.0))
            }),
        });
    }
    if subscriptions
        .iter()
        .any(|subscription| subscription.status == "VALID" && subscription.in_current_period)
    {
        (
            accepted(source, checked_at),
            EntitlementState::Available,
            subscriptions,
        )
    } else if malformed_coding {
        (
            malformed(source, checked_at),
            EntitlementState::Unknown,
            subscriptions,
        )
    } else {
        (
            accepted(source, checked_at),
            EntitlementState::Unavailable,
            subscriptions,
        )
    }
}
fn parse_coding_quota(
    value: &Value,
    credentials: &Credentials,
    source: Option<CredentialSource>,
    checked_at: u64,
) -> (CheckResult, Vec<CodingLimit>) {
    let Some(list) = value.get("limits").and_then(Value::as_array) else {
        return (malformed(source, checked_at), Vec::new());
    };
    let mut limits = Vec::new();
    let mut malformed_limit = false;
    for value in list {
        let Some(limit_type) = credentials.text(value, "type") else {
            malformed_limit = true;
            continue;
        };
        if !numeric_fields_valid(
            value,
            &[
                "unit",
                "number",
                "usage",
                "currentValue",
                "remaining",
                "percentage",
                "nextResetTime",
            ],
            false,
        ) {
            malformed_limit = true;
            continue;
        }
        let usage_details = value
            .get("usageDetails")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter(|value| value.is_object())
                    .map(|value| CodingUsageDetail {
                        model_code: credentials.text(value, "modelCode"),
                        display_name: credentials.text(value, "displayName"),
                        usage: number(value, "usage", false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        limits.push(CodingLimit {
            limit_type,
            unit: number(value, "unit", false),
            number: number(value, "number", false),
            usage: number(value, "usage", false),
            current_value: number(value, "currentValue", false),
            remaining: number(value, "remaining", false),
            percentage: number(value, "percentage", false),
            next_reset_time_ms: number(value, "nextResetTime", false),
            usage_details,
        });
    }
    (
        if malformed_limit {
            malformed(source, checked_at)
        } else {
            accepted(source, checked_at)
        },
        limits,
    )
}
#[cfg(test)]
#[path = "session_checks_tests.rs"]
mod tests;
