//! Explicit ongoing consent in the original encrypted Provider owner. The
//! original campaign ledger remains the only claim/anti-replay authority.
use super::*;
use std::{future::Future, task::Poll, time::Duration};

pub(crate) const POLICY_VERSION: u32 = 1;
pub(crate) const MAX_REVISION: u64 = 9_007_199_254_740_991;
pub(crate) const STARTUP_DELAY: Duration = Duration::from_secs(30);
pub(crate) const INTERVAL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Policy {
    pub(crate) enabled: bool,
    pub(crate) revision: u64,
    pub(crate) policy_version: u32,
    pub(crate) subject: Option<String>,
    pub(crate) credential_generation: Option<String>,
}
impl Policy {
    pub(crate) fn revoke(&mut self) {
        self.enabled = false;
        self.credential_generation = None;
        self.revision = self.revision.saturating_add(1).min(MAX_REVISION);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    pub provider_id: String,
    pub credential_generation: String,
    pub expected_revision: u64,
    pub policy_version: u32,
    pub enabled: bool,
    pub source: Option<http::CredentialSource>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Context {
    pub provider_id: String,
    pub credential_generation: String,
    pub revision: u64,
    pub policy_version: u32,
    pub enabled: bool,
    pub can_enable: bool,
    pub reason: Option<String>,
    pub subject: Option<String>,
}
pub(crate) fn denied() -> AppError {
    AppError::Config("qoder.automatic_authorization_changed".into())
}
fn source_supported(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|m| m.provider_type.as_deref())
        == Some("qoder_sdk")
        && !crate::services::vendor_source_binding::is_reference(provider)
        && credential_candidate(provider)
            .ok()
            .flatten()
            .is_some_and(|key| !key.trim().is_empty())
}
pub(crate) fn active(provider: &Provider) -> Option<&Policy> {
    let state = provider.meta.as_ref()?.qoder_campaign_state.as_ref()?;
    let policy = &state.automatic;
    (source_supported(provider)
        && policy.enabled
        && policy.revision <= MAX_REVISION
        && policy.policy_version == POLICY_VERSION
        && policy.credential_generation.as_deref() == Some(state.generation.as_str())
        && policy
            .subject
            .as_ref()
            .is_some_and(|uid| !uid.trim().is_empty()))
    .then_some(policy)
}
fn view(provider: &Provider, device: Option<Dependency>) -> Result<Context, AppError> {
    let state = provider
        .meta
        .as_ref()
        .and_then(|m| m.qoder_campaign_state.as_ref())
        .ok_or_else(denied)?;
    let reason = if !source_supported(provider) {
        Some("SourceUnavailable".to_owned())
    } else {
        device.map(|reason| format!("{reason:?}"))
    };
    Ok(Context {
        provider_id: provider.id.clone(),
        credential_generation: state.generation.clone(),
        revision: state.automatic.revision,
        policy_version: POLICY_VERSION,
        enabled: active(provider).is_some(),
        can_enable: reason.is_none() && state.automatic.revision < MAX_REVISION,
        reason,
        subject: state.automatic.subject.clone(),
    })
}
pub(crate) fn context(db: &Database, provider_id: &str) -> Result<Context, AppError> {
    use device::DeviceSource;
    let provider = initialize_provider(db, provider_id)?;
    view(&provider, device::InstalledApp.availability().err())
}

/// This source declaration belongs to the explicit enrollment action only.
/// Subsequent rounds consume a different, backend-only continuous authorization.
pub(crate) fn set(db: &Database, request: Request) -> Result<Context, AppError> {
    let declaration = request.enabled.then(|| http::PatDeclaration {
        provider_id: request.provider_id.clone(),
        credential_generation: request.credential_generation.clone(),
        source: http::CredentialSource::InternationalQoderPat,
    });
    let adapter = http::HttpAdapter::production(declaration)?;
    set_with_adapter(db, request, &adapter)
}
pub(crate) fn set_with_adapter(
    db: &Database,
    request: Request,
    adapter: &dyn CampaignAdapter,
) -> Result<Context, AppError> {
    let epoch = db.secrets.lifecycle_epoch();
    let provider = initialize_provider(db, &request.provider_id)?;
    let state = provider
        .meta
        .as_ref()
        .and_then(|m| m.qoder_campaign_state.as_ref())
        .ok_or_else(denied)?;
    if state.generation != request.credential_generation
        || state.automatic.revision != request.expected_revision
        || request.policy_version != POLICY_VERSION
        || request.expected_revision >= MAX_REVISION
        || (request.enabled
            && (request.source != Some(http::CredentialSource::InternationalQoderPat)
                || !source_supported(&provider)))
    {
        return Err(denied());
    }
    let subject = if request.enabled {
        let verified = adapter
            .verify(&provider, &state.generation)
            .map_err(|_| denied())?;
        if verified.generation != state.generation
            || verified.identity.provider_id != provider.id
            || verified.identity.region != Region::International
            || verified.identity.uid.trim().is_empty()
            || !verified.official_device
        {
            return Err(denied());
        }
        Some(verified.identity.uid)
    } else {
        None
    };
    let provider = db.save_qoder_automatic(&request, subject, epoch)?;
    use device::DeviceSource;
    view(&provider, device::InstalledApp.availability().err())
}

#[derive(Clone)]
pub(crate) struct Lease {
    pub(crate) provider_id: String,
    pub(crate) generation: String,
    pub(crate) revision: u64,
    pub(crate) subject: String,
    pub(crate) epoch: u64,
}
impl Lease {
    pub(crate) fn from_provider(provider: &Provider, epoch: u64) -> Result<Self, AppError> {
        let policy = active(provider).ok_or_else(denied)?;
        Ok(Self {
            provider_id: provider.id.clone(),
            generation: policy.credential_generation.clone().ok_or_else(denied)?,
            revision: policy.revision,
            subject: policy.subject.clone().ok_or_else(denied)?,
            epoch,
        })
    }
    pub(crate) fn matches(&self, provider: &Provider) -> bool {
        provider.id == self.provider_id
            && active(provider).is_some_and(|policy| {
                policy.revision == self.revision
                    && policy.subject.as_deref() == Some(self.subject.as_str())
                    && policy.credential_generation.as_deref() == Some(self.generation.as_str())
            })
    }
}
pub(crate) struct Admission<'a> {
    pub(crate) db: &'a Database,
    pub(crate) lease: Lease,
}
impl Admission<'_> {
    pub(crate) fn matches(&self, context: &VerifiedContext) -> bool {
        context.identity.provider_id == self.lease.provider_id
            && context.identity.uid == self.lease.subject
            && context.identity.region == Region::International
            && context.generation == self.lease.generation
    }
    pub(crate) fn dispatch<T>(
        &self,
        operation: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        self.db
            .with_qoder_automatic_admission(&self.lease, operation)
    }
    pub(crate) fn dispatch_claim<T>(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
        observation: &clock::Observation,
        operation: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        self.dispatch(|| {
            // The same authenticated Date observation must still be valid after
            // waiting for the vault/SQLite barriers and decrypting current rows.
            if !self.matches(context)
                || !observation.window_open(context, binding.start_at(), binding.end_at())
            {
                return Err(AppError::Config("qoder.campaign_window_closed".into()));
            }
            operation()
        })
    }
    /// Creation and first poll are admitted while the original SQLite/vault
    /// owners are held. No owner/credential guard is held over a network wait.
    pub(crate) async fn send<F>(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
        observation: &clock::Observation,
        create: impl FnOnce() -> F,
    ) -> Result<reqwest::Response, AppError>
    where
        F: Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        let waker = std::future::poll_fn(|cx| Poll::Ready(cx.waker().clone())).await;
        let (mut future, first) = self.dispatch_claim(context, binding, observation, || {
            let mut future = Box::pin(create());
            let first = future
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&waker));
            Ok((future, first))
        })?;
        match first {
            Poll::Ready(result) => result.map_err(|_| unavailable()),
            Poll::Pending => future.as_mut().await.map_err(|_| unavailable()),
        }
    }
}
pub(crate) fn run_with_adapter(
    db: &Database,
    lease: Lease,
    adapter: &dyn CampaignAdapter,
) -> Result<ClaimResult, AppError> {
    let id = lease.provider_id.clone();
    let admission = Admission { db, lease };
    prepare_selected_admitted(db, &id, None, adapter, Some(&admission))
}
pub(crate) fn tick(db: &Database) -> Result<(), AppError> {
    let epoch = db.secrets.lifecycle_epoch();
    let providers = db.get_all_providers("codex")?;
    for provider in providers.values() {
        let Ok(lease) = Lease::from_provider(provider, epoch) else {
            continue;
        };
        let adapter = http::HttpAdapter::continuous(lease.clone())?;
        // Per-account failures are bounded by the next maintenance interval.
        // Unknown outcomes remain GET-only in the original claim owner.
        let _ = run_with_adapter(db, lease, &adapter);
    }
    Ok(())
}
