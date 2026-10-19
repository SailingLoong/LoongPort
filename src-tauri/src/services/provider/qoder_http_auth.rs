//! Backend-only HTTP credential owner. No production transport or admission is enabled.
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
pub(crate) const PRODUCTION_ADMISSION: bool = false;
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
}
impl SourceBinding {
    pub(crate) fn from_provider(
        provider: &Provider,
    ) -> Result<(Self, Zeroizing<String>), AppError> {
        if provider
            .meta
            .as_ref()
            .and_then(|m| m.provider_type.as_deref())
            != Some("qoder_sdk")
        {
            return Err(denied("unsupported_source"));
        }
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
                source: SourceKind::InternationalQoderPat,
                origin: ORIGIN.into(),
                credential_generation: generation,
                credential_digest: hex::encode(digest.finalize()),
            },
            pat,
        ))
    }
    pub(crate) fn admit(&self, declaration: &PatDeclaration) -> Result<(), AppError> {
        if declaration.provider_id != self.provider_id
            || declaration.credential_generation != self.credential_generation
            || declaration.source != CredentialSource::InternationalQoderPat
        {
            return Err(denied("source_declaration_required"));
        }
        Ok(())
    }
    fn validate(&self) -> bool {
        self.app_type == "codex"
            && self.origin == ORIGIN
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
/// Backend-private receipt captured when requesting explicit recovery approval.
/// A delayed approval may only retire this exact attempt, never the current one.
pub(crate) struct RecoveryReceipt {
    pub(crate) stamp: Stamp,
    pub(crate) attempt_id: String,
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
        "international"
    }
    pub(crate) fn origin(&self) -> &'static str {
        ORIGIN
    }
    pub(crate) fn generation(&self) -> u64 {
        self.stamp.generation
    }
    pub(crate) fn source_generation(&self) -> &str {
        &self.stamp.source.credential_generation
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

/// This service performs no automatic PAT fallback or background exchange.
/// Cancellation during await leaves a durable non-replayable attempt.
pub(crate) async fn establish(
    db: &Database,
    declaration: &PatDeclaration,
    intent: EstablishIntent,
    transport: &dyn AuthTransport,
    now_ms: impl Fn() -> i64,
) -> Result<RequestSnapshot, AppError> {
    let lease = db.qoder_http_begin_establish(declaration, intent)?;
    let response = transport
        .exchange_and_identify(&lease.pat, &lease.machine_id)
        .await;
    match response {
        Ok(response) => db.qoder_http_publish_establish(&lease, response, now_ms()),
        Err(()) => {
            db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)?;
            Err(denied("outcome_unknown_reestablish_required"))
        }
    }
}
/// Retry eligibility is a stage fact, not a string category. A missing PAT
/// observed after a submitted refresh must never authorize another candidate.
pub(crate) enum AcquireError {
    PreflightMissingSource,
    NonRetryable(AcquireFailure),
}
pub(crate) struct AcquireFailure {
    // Retained inside the owner only; never expose a post-submission cause as a
    // retry hint or serialize it to a caller. Tests can inspect the exact cause.
    #[allow(dead_code)]
    cause: AppError,
}
impl AcquireError {
    fn preflight(cause: AppError) -> Self {
        if matches!(&cause, AppError::Config(code) if matches!(code.as_str(),
            "qoder.http_auth.source_required" | "qoder.http_auth.source_declaration_required"))
        {
            Self::PreflightMissingSource
        } else {
            Self::non_retryable(cause)
        }
    }
    fn non_retryable(cause: AppError) -> Self {
        Self::NonRetryable(AcquireFailure { cause })
    }
}
impl std::fmt::Debug for AcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PreflightMissingSource => "PreflightMissingSource",
            Self::NonRetryable(_) => "NonRetryable",
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
                    .map_err(AcquireError::non_retryable),
                Err(()) => {
                    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
                        .map_err(AcquireError::non_retryable)?;
                    Err(AcquireError::non_retryable(denied(
                        "outcome_unknown_reestablish_required",
                    )))
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
