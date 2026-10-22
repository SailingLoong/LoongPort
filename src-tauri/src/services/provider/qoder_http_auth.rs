//! Backend-owned HTTP credentials. Admission requires an explicit source declaration
//! and a current identity in the original encrypted Provider owner.
//! The configured PAT remains in Provider; only its derived state occupies the
//! encrypted subordinate column. No credential type here implements Debug.
use super::qoder_daily::{
    self,
    http::{CredentialSource, PatDeclaration},
};
use crate::{database::Database, error::AppError, provider::Provider};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin};
use zeroize::{Zeroize, Zeroizing};

pub(crate) const ORIGIN: &str = "https://openapi.qoder.sh";
const FORMAT: u32 = 1;
pub(crate) const REFRESH_MARGIN_MS: i64 = 300_000;

pub(crate) fn denied(code: &str) -> AppError {
    AppError::Config(format!("qoder.http_auth.{code}"))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceBinding {
    pub(crate) provider_id: String,
    pub(crate) app_type: String,
    pub(crate) source: SourceKind,
    pub(crate) origin: String,
    pub(crate) credential_generation: String,
    pub(crate) credential_digest: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SourceKind {
    InternationalQoderPat,
    ChinaQoderPat,
}
impl SourceKind {
    pub(crate) fn from_provider(provider: &Provider) -> Result<Self, AppError> {
        match provider
            .meta
            .as_ref()
            .and_then(|m| m.provider_type.as_deref())
        {
            Some("qoder_sdk") => Ok(Self::InternationalQoderPat),
            Some("qoder_cn") => Ok(Self::ChinaQoderPat),
            _ => Err(denied("unsupported_source")),
        }
    }
    pub(crate) fn origin(&self) -> &'static str {
        match self {
            Self::InternationalQoderPat => ORIGIN,
            Self::ChinaQoderPat => "https://openapi.qoder.com.cn",
        }
    }
    pub(crate) fn declaration(&self) -> CredentialSource {
        match self {
            Self::InternationalQoderPat => CredentialSource::InternationalQoderPat,
            Self::ChinaQoderPat => CredentialSource::ChinaQoderPat,
        }
    }
}
impl SourceBinding {
    pub(crate) fn from_provider(
        provider: &Provider,
    ) -> Result<(Self, Zeroizing<String>), AppError> {
        let source = SourceKind::from_provider(provider)?;
        if crate::services::vendor_source_binding::is_reference(provider) {
            return Err(denied("external_source_untracked"));
        }
        let generation = provider
            .meta
            .as_ref()
            .and_then(|m| m.qoder_campaign_state.as_ref())
            .map(|s| s.generation.clone())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| denied("source_generation_required"))?;
        let pat = Zeroizing::new(
            qoder_daily::credential_candidate(provider)
                .map_err(|_| denied("external_source_untracked"))?
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| denied("source_required"))?,
        );
        let mut digest = Sha256::new();
        digest.update(b"loongport.qoder-http.pat.v1\0");
        digest.update(pat.as_bytes());
        Ok((
            Self {
                provider_id: provider.id.clone(),
                app_type: "codex".into(),
                origin: source.origin().into(),
                source,
                credential_generation: generation,
                credential_digest: hex::encode(digest.finalize()),
            },
            pat,
        ))
    }
    pub(crate) fn admit(&self, declaration: &PatDeclaration) -> Result<(), AppError> {
        if declaration.provider_id != self.provider_id
            || declaration.credential_generation != self.credential_generation
            || declaration.source != self.source.declaration()
        {
            return Err(denied("source_declaration_required"));
        }
        Ok(())
    }
    fn validate(&self) -> bool {
        self.app_type == "codex"
            && self.origin == self.source.origin()
            && !self.provider_id.is_empty()
            && !self.credential_generation.is_empty()
            && self.credential_digest.len() == 64
            && self
                .credential_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Identity {
    pub(crate) uid: String,
    /// COSY identity only. Never the native daily campaign device contract.
    pub(crate) machine_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TokenPair {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_at_ms: i64,
}
impl Drop for TokenPair {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
impl TokenPair {
    fn validate(&self, now_ms: i64) -> Result<(), AppError> {
        if self.access_token.trim().is_empty() || self.expires_at_ms <= now_ms || now_ms < 0 {
            return Err(denied("invalid_token_response"));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum Phase {
    Establishing {
        attempt_id: String,
        expected_uid: Option<String>,
        machine_id: String,
    },
    Ready {
        identity: Identity,
        tokens: TokenPair,
    },
    // The consumed refresh token is intentionally absent from durable Pending
    // and Unknown states. Only the in-memory original-owner lease holds it.
    RefreshPending {
        attempt_id: String,
        identity: Identity,
    },
    Unknown {
        attempt_id: String,
        identity: Option<Identity>,
        machine_id: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) format: u32,
    pub(crate) source: SourceBinding,
    pub(crate) generation: u64,
    pub(crate) phase: Phase,
}
impl Record {
    pub(crate) fn decode(raw: &str) -> Result<Option<Self>, AppError> {
        if raw.is_empty() || raw.trim() == "{}" {
            return Ok(None);
        }
        let record: Self = serde_json::from_str(raw).map_err(|_| denied("invalid_record"))?;
        let valid_id = |id: &Identity| {
            !id.uid.trim().is_empty() && uuid::Uuid::parse_str(&id.machine_id).is_ok()
        };
        let valid = match &record.phase {
            Phase::Establishing {
                attempt_id,
                machine_id,
                expected_uid,
            } => {
                uuid::Uuid::parse_str(attempt_id).is_ok()
                    && uuid::Uuid::parse_str(machine_id).is_ok()
                    && expected_uid
                        .as_ref()
                        .is_none_or(|uid| !uid.trim().is_empty())
            }
            Phase::Ready { identity, tokens } => valid_id(identity) && tokens.validate(0).is_ok(),
            Phase::RefreshPending {
                attempt_id,
                identity,
            } => uuid::Uuid::parse_str(attempt_id).is_ok() && valid_id(identity),
            Phase::Unknown {
                attempt_id,
                identity,
                machine_id,
            } => {
                uuid::Uuid::parse_str(attempt_id).is_ok()
                    && uuid::Uuid::parse_str(machine_id).is_ok()
                    && identity.as_ref().is_none_or(valid_id)
                    && identity
                        .as_ref()
                        .is_none_or(|i| i.machine_id == *machine_id)
            }
        };
        if record.format != FORMAT || record.generation == 0 || !record.source.validate() || !valid
        {
            return Err(denied("invalid_record"));
        }
        Ok(Some(record))
    }
    pub(crate) fn new(source: SourceBinding, generation: u64, phase: Phase) -> Self {
        Self {
            format: FORMAT,
            source,
            generation,
            phase,
        }
    }
    pub(crate) fn next_generation(&self) -> Result<u64, AppError> {
        self.generation
            .checked_add(1)
            .ok_or_else(|| denied("generation_exhausted"))
    }
    pub(crate) fn identity_and_machine(&self) -> (Option<Identity>, String) {
        match &self.phase {
            Phase::Ready { identity, .. } | Phase::RefreshPending { identity, .. } => {
                (Some(identity.clone()), identity.machine_id.clone())
            }
            Phase::Unknown {
                identity,
                machine_id,
                ..
            } => (identity.clone(), machine_id.clone()),
            Phase::Establishing {
                expected_uid,
                machine_id,
                ..
            } => (
                expected_uid.as_ref().map(|uid| Identity {
                    uid: uid.clone(),
                    machine_id: machine_id.clone(),
                }),
                machine_id.clone(),
            ),
        }
    }
}

/// SameIdentity is the only retry after an unknown result. Changing a known UID
/// requires an explicit service action naming the previous authenticated UID.
pub(crate) enum EstablishIntent {
    SameIdentity,
    Rebind { previous_uid: String },
}
pub(crate) struct Stamp {
    pub(crate) source: SourceBinding,
    pub(crate) generation: u64,
    pub(crate) vault_epoch: u64,
}
pub(crate) struct EstablishLease {
    pub(crate) stamp: Stamp,
    pub(crate) attempt_id: String,
    pub(crate) expected_uid: Option<String>,
    pub(crate) machine_id: String,
    pub(crate) pat: Zeroizing<String>,
}
pub(crate) struct RefreshLease {
    pub(crate) stamp: Stamp,
    pub(crate) attempt_id: String,
    pub(crate) identity: Identity,
    pub(crate) refresh_token: Zeroizing<String>,
}
/// Request-scoped only. No Serialize, Deserialize or Debug, and no refresh token.
/// The proxy must recheck this fence immediately before emitting its private pipe frame.
pub(crate) struct RequestSnapshot {
    pub(crate) stamp: Stamp,
    identity: Identity,
    access_token: Zeroizing<String>,
    expires_at_ms: i64,
}
impl RequestSnapshot {
    pub(crate) fn new(stamp: Stamp, identity: Identity, tokens: &TokenPair) -> Self {
        Self {
            stamp,
            identity,
            access_token: Zeroizing::new(tokens.access_token.clone()),
            expires_at_ms: tokens.expires_at_ms,
        }
    }
    pub(crate) fn access_token(&self) -> &str {
        &self.access_token
    }
    pub(crate) fn uid(&self) -> &str {
        &self.identity.uid
    }
    pub(crate) fn machine_id(&self) -> &str {
        &self.identity.machine_id
    }
    pub(crate) fn region(&self) -> &'static str {
        match self.stamp.source.source {
            SourceKind::InternationalQoderPat => "international",
            SourceKind::ChinaQoderPat => "cn",
        }
    }
    pub(crate) fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }
}
pub(crate) enum Acquisition {
    Ready(RequestSnapshot),
    Refresh(RefreshLease),
}

/// Implemented only by the original backend service's authenticated transport.
/// The network worker used for inference must never implement this trait.
pub(crate) trait AuthTransport: Send + Sync {
    fn source_kind(&self) -> SourceKind {
        SourceKind::InternationalQoderPat
    }
    fn exchange_and_identify<'a>(
        &'a self,
        pat: &'a str,
        machine_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Established, ()>> + Send + 'a>>;
    fn refresh<'a>(
        &'a self,
        refresh_token: &'a str,
        identity: &'a Identity,
    ) -> Pin<Box<dyn Future<Output = Result<TokenPair, ()>> + Send + 'a>>;
}
pub(crate) struct Established {
    pub(crate) uid: String,
    pub(crate) tokens: TokenPair,
}

/// Retry eligibility is a stage fact, not a string category. A missing PAT
/// observed after a submitted refresh must never authorize another candidate.
pub(crate) enum AcquireError {
    PreflightMissingSource,
    NonRetryable,
}
impl AcquireError {
    fn preflight(cause: AppError) -> Self {
        if matches!(&cause, AppError::Config(code) if matches!(code.as_str(),
            "qoder.http_auth.source_required" | "qoder.http_auth.source_declaration_required"))
        {
            Self::PreflightMissingSource
        } else {
            Self::NonRetryable
        }
    }
}
impl std::fmt::Debug for AcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PreflightMissingSource => "PreflightMissingSource",
            Self::NonRetryable => "NonRetryable",
        })
    }
}

pub(crate) async fn acquire(
    db: &Database,
    selected: &Provider,
    transport: &dyn AuthTransport,
    now_ms: impl Fn() -> i64,
) -> Result<RequestSnapshot, AcquireError> {
    let (expected, _) = SourceBinding::from_provider(selected).map_err(AcquireError::preflight)?;
    if transport.source_kind() != expected.source {
        return Err(AcquireError::NonRetryable);
    }
    // Only these synchronous source/acquisition checks may classify a
    // zero-network preflight failure. Never reuse the classifier for
    // publication, cleanup or snapshot fences.
    match db
        .qoder_http_acquire(&expected, now_ms())
        .map_err(AcquireError::preflight)?
    {
        Acquisition::Ready(snapshot) => Ok(snapshot),
        Acquisition::Refresh(lease) => {
            let response = transport
                .refresh(&lease.refresh_token, &lease.identity)
                .await;
            match response {
                Ok(tokens) => db
                    .qoder_http_publish_refresh(&lease, tokens, now_ms())
                    .map_err(|_| AcquireError::NonRetryable),
                Err(()) => {
                    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
                        .map_err(|_| AcquireError::NonRetryable)?;
                    Err(AcquireError::NonRetryable)
                }
            }
        }
    }
}

pub(crate) fn validate_tokens(tokens: &TokenPair, now_ms: i64) -> Result<(), AppError> {
    tokens.validate(now_ms)
}

#[cfg(test)]
#[path = "qoder_http_auth_tests.rs"]
mod tests;

/// Compare the already-selected original request Provider with this acquired
/// source. A stale selected request cannot borrow a newer PAT after awaiting.
pub(crate) fn check_selected_provider(
    provider: &Provider,
    snapshot: &RequestSnapshot,
) -> Result<(), AppError> {
    let (selected, _) = SourceBinding::from_provider(provider)?;
    if selected != snapshot.stamp.source {
        return Err(denied("selected_source_changed"));
    }
    Ok(())
}

#[path = "qoder_http_auth/transport.rs"]
pub(crate) mod transport;

// The only frontend projection. Private credentials/receipts never implement
// frontend serialization or appear inside one of these explicitly safe DTOs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum IdentityAction {
    Establish,
    Recover,
    Rebind,
}
pub(crate) struct IdentityActionRequest {
    pub(crate) declaration: PatDeclaration,
    pub(crate) action: IdentityAction,
}
impl<'de> Deserialize<'de> for IdentityActionRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            declaration: PatDeclaration,
            action: IdentityAction,
        }
        // Even an invalid enum value must not be echoed as a transport/IPC error.
        let wire = Wire::deserialize(deserializer)
            .map_err(|_| serde::de::Error::custom("qoder.http_identity.invalid_request"))?;
        Ok(Self {
            declaration: wire.declaration,
            action: wire.action,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum IdentityStatus {
    Unavailable,
    Unestablished,
    Ready,
    RefreshNeeded,
    ReestablishRequired,
    Pending,
    Unknown,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IdentityContext {
    pub(crate) provider_id: String,
    pub(crate) credential_generation: String,
    pub(crate) source: Option<CredentialSource>,
    pub(crate) status: IdentityStatus,
    pub(crate) expires_at_ms: Option<i64>,
    pub(crate) allowed_actions: Vec<IdentityAction>,
}
impl IdentityContext {
    pub(crate) fn unavailable(provider_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            credential_generation: String::new(),
            source: None,
            status: IdentityStatus::Unavailable,
            expires_at_ms: None,
            allowed_actions: Vec::new(),
        }
    }
    fn ready(source: &SourceBinding, expires_at_ms: i64, refreshable: bool, now_ms: i64) -> Self {
        let status = if expires_at_ms > now_ms.saturating_add(REFRESH_MARGIN_MS) {
            IdentityStatus::Ready
        } else if refreshable {
            IdentityStatus::RefreshNeeded
        } else {
            IdentityStatus::ReestablishRequired
        };
        let mut allowed_actions = if status == IdentityStatus::ReestablishRequired {
            vec![IdentityAction::Establish]
        } else {
            Vec::new()
        };
        allowed_actions.push(IdentityAction::Rebind);
        Self {
            provider_id: source.provider_id.clone(),
            credential_generation: source.credential_generation.clone(),
            source: Some(source.source.declaration()),
            status,
            expires_at_ms: Some(expires_at_ms),
            allowed_actions,
        }
    }
    pub(crate) fn from_record(
        source: &SourceBinding,
        record: Option<&Record>,
        now_ms: i64,
    ) -> Self {
        let (status, expires_at_ms, mut allowed_actions) = match record.map(|r| &r.phase) {
            None => (
                IdentityStatus::Unestablished,
                None,
                vec![IdentityAction::Establish],
            ),
            Some(Phase::Ready { tokens, .. }) => {
                return Self::ready(
                    source,
                    tokens.expires_at_ms,
                    !tokens.refresh_token.trim().is_empty(),
                    now_ms,
                );
            }
            Some(Phase::Establishing { .. } | Phase::RefreshPending { .. }) => {
                (IdentityStatus::Pending, None, vec![IdentityAction::Recover])
            }
            Some(Phase::Unknown { .. }) => {
                (IdentityStatus::Unknown, None, vec![IdentityAction::Recover])
            }
        };
        if record.is_some_and(|r| r.identity_and_machine().0.is_some()) {
            allowed_actions.push(IdentityAction::Rebind);
        }
        Self {
            provider_id: source.provider_id.clone(),
            credential_generation: source.credential_generation.clone(),
            source: Some(source.source.declaration()),
            status,
            expires_at_ms,
            allowed_actions,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityFailureCode {
    Unavailable,
    SourceChanged,
    OwnerChanged,
    InvalidSource,
    StateChanged,
    IdentityRebindRequired,
    OutcomeUnknown,
}
impl IdentityFailureCode {
    fn from_error(error: &AppError) -> Self {
        match error {
            AppError::Config(code) => match code.strip_prefix("qoder.http_auth.") {
                Some(
                    "source_changed" | "source_declaration_required" | "selected_source_changed",
                ) => Self::SourceChanged,
                Some("stale_owner") => Self::OwnerChanged,
                Some(
                    "unsupported_source"
                    | "external_source_untracked"
                    | "source_generation_required"
                    | "source_required",
                ) => Self::InvalidSource,
                Some("identity_rebind_required") => Self::IdentityRebindRequired,
                Some("attempt_in_progress" | "no_pending_attempt" | "rebind_subject_changed") => {
                    Self::StateChanged
                }
                Some("outcome_unknown_reestablish_required" | "invalid_token_response") => {
                    Self::OutcomeUnknown
                }
                _ => Self::Unavailable,
            },
            _ => Self::Unavailable,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum IdentityActionResult {
    Completed { context: IdentityContext },
    Changed { context: IdentityContext },
    Cancelled,
    Failed { code: IdentityFailureCode },
}
impl IdentityActionResult {
    pub(crate) fn unavailable() -> Self {
        Self::Failed {
            code: IdentityFailureCode::Unavailable,
        }
    }
    fn error(error: &AppError) -> Self {
        Self::Failed {
            code: IdentityFailureCode::from_error(error),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum IdentityConfirmationKind {
    Recover,
    Rebind,
}
pub(crate) struct IdentityConfirmation {
    pub(crate) kind: IdentityConfirmationKind,
    pub(crate) provider_name: String,
}
#[derive(PartialEq, Eq)]
pub(crate) enum IdentityPhaseStamp {
    Absent,
    Establishing(String),
    Ready,
    RefreshPending(String),
    Unknown(String),
}
impl IdentityPhaseStamp {
    pub(crate) fn of(record: Option<&Record>) -> Self {
        match record.map(|r| &r.phase) {
            None => Self::Absent,
            Some(Phase::Ready { .. }) => Self::Ready,
            Some(Phase::Establishing { attempt_id, .. }) => Self::Establishing(attempt_id.clone()),
            Some(Phase::RefreshPending { attempt_id, .. }) => {
                Self::RefreshPending(attempt_id.clone())
            }
            Some(Phase::Unknown { attempt_id, .. }) => Self::Unknown(attempt_id.clone()),
        }
    }
}
/// Never serialized, cloned or retained in a registry. Absence and phase are
/// fenced as well as generation so a completed attempt invalidates old consent.
pub(crate) struct IdentityActionReceipt {
    pub(crate) source: SourceBinding,
    pub(crate) vault_epoch: u64,
    pub(crate) generation: Option<u64>,
    pub(crate) phase: IdentityPhaseStamp,
    pub(crate) intent: EstablishIntent,
}
pub(crate) enum PreparedIdentityAction {
    Complete(IdentityContext),
    Changed(IdentityContext),
    Establish {
        receipt: IdentityActionReceipt,
        confirmation: Option<IdentityConfirmation>,
    },
}

pub(crate) fn identity_context(db: &Database, provider_id: &str, now_ms: i64) -> IdentityContext {
    identity_context_admitted(db, provider_id, now_ms)
        .unwrap_or_else(|_| IdentityContext::unavailable(provider_id))
}
fn identity_context_admitted(
    db: &Database,
    provider_id: &str,
    now_ms: i64,
) -> Result<IdentityContext, AppError> {
    if now_ms < 0 {
        return Err(denied("clock_invalid"));
    }
    // Only the original source revision initializer is shared with Daily.
    // Never call its device-dependent ClaimContext or perform a campaign action.
    qoder_daily::initialize_provider(db, provider_id)?;
    db.qoder_http_identity_context(provider_id, now_ms)
}

pub(crate) async fn perform_identity_action<F, C>(
    db: &Database,
    request: IdentityActionRequest,
    transport: &dyn AuthTransport,
    confirm: C,
    now_ms: impl Fn() -> i64,
) -> IdentityActionResult
where
    C: FnOnce(IdentityConfirmation) -> F,
    F: Future<Output = bool>,
{
    let prepared = match db.qoder_http_prepare_identity_action(&request, now_ms()) {
        Ok(prepared) => prepared,
        Err(error) => return IdentityActionResult::error(&error),
    };
    let (receipt, confirmation) = match prepared {
        PreparedIdentityAction::Complete(context) => {
            return IdentityActionResult::Completed { context }
        }
        PreparedIdentityAction::Changed(context) => {
            return IdentityActionResult::Changed { context }
        }
        PreparedIdentityAction::Establish {
            receipt,
            confirmation,
        } => (receipt, confirmation),
    };
    if transport.source_kind() != receipt.source.source {
        return IdentityActionResult::Failed {
            code: IdentityFailureCode::InvalidSource,
        };
    }
    // Preparation released every guard. No refresh/PAT is in the confirmation.
    if let Some(summary) = confirmation {
        if !confirm(summary).await {
            return IdentityActionResult::Cancelled;
        }
    }
    let lease = match db.qoder_http_reserve_identity_action(receipt) {
        Ok(lease) => lease,
        Err(error) => return IdentityActionResult::error(&error),
    };
    // One reservation, one exchange/identify, no automatic retry or refresh.
    let response = transport
        .exchange_and_identify(&lease.pat, &lease.machine_id)
        .await;
    match response {
        Ok(response) => {
            let now = now_ms();
            // Safe result is derived from this exact response, not a later row
            // lookup that could borrow another source after publication.
            let context = IdentityContext::ready(
                &lease.stamp.source,
                response.tokens.expires_at_ms,
                !response.tokens.refresh_token.trim().is_empty(),
                now,
            );
            match db.qoder_http_publish_establish(&lease, response, now) {
                Ok(_) => IdentityActionResult::Completed { context },
                Err(error) => {
                    let _ = db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id);
                    IdentityActionResult::error(&error)
                }
            }
        }
        Err(()) => {
            let _ = db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id);
            IdentityActionResult::Failed {
                code: IdentityFailureCode::OutcomeUnknown,
            }
        }
    }
}

#[cfg(test)]
#[path = "qoder_http_identity_tests.rs"]
mod identity_tests;
