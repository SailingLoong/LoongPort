//! Explicit, backend-owned daily claim orchestration. Never invoked by usage/chat.
//! Explicit source declaration, authenticated campaign subject and local device
//! dependency are resolved before the existing SQLite owner grants one POST.
use crate::{database::Database, error::AppError, provider::Provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Instant;
#[path = "qoder_daily/contract.rs"]
pub(crate) mod contract;
use contract::{BoundCampaign, Identity, Outcome, Region, Target};
#[path = "qoder_daily/automatic.rs"]
pub(crate) mod automatic;
#[path = "qoder_daily/clock.rs"]
pub(crate) mod clock;
#[path = "qoder_daily/device.rs"]
pub(crate) mod device;
#[path = "qoder_daily/http.rs"]
pub(crate) mod http;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct State {
    pub generation: String,
    pub pending: Option<Attempt>,
    #[serde(default)]
    pub(crate) automatic: automatic::Policy,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attempt {
    pub id: String,
    pub generation: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Operation {
    pub id: String,
    pub generation: String,
    pub binding: BoundCampaign,
    pub outcome: Outcome,
}
impl State {
    pub(crate) fn fresh() -> Self {
        Self {
            generation: uuid::Uuid::new_v4().to_string(),
            pending: None,
            automatic: automatic::Policy::default(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistOutcome {
    Applied,
    AlreadyCurrent,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Dependency {
    IdentityUnavailable,
    AuthFamilyUnverified,
    OfficialDeviceUnavailable,
    OfficialDeviceUnverified,
    OfficialDeviceUnsupported,
    OfficialDeviceFailed,
    AuthenticationFailed,
    ReceiptOwnerUnavailable,
    ClockUnverified,
    IdentityMismatch,
    ExternalCredentialSourceUntracked,
    CampaignWindowClosed,
    NoEligibleCampaign,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum Action {
    None,
    Post,
    Get,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum ClaimResult {
    Unavailable(Dependency),
    Observed {
        action: Action,
        outcome: Outcome,
        added_credits: Option<u16>,
        operation: ObservedOperation,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ObservedOperation {
    pub attempt_id: String,
    pub campaign_id: String,
    pub campaign_key: String,
    pub start_at: i64,
    pub end_at: i64,
}
// No Deserialize or public fields. Only the authenticated transport binds the
// declared credential to a campaign subject and a device dependency.
pub(crate) struct VerifiedContext {
    identity: Identity,
    generation: String,
    official_device: bool,
    origin: reqwest::Url,
    session: Option<http::Session>,
}
#[cfg(test)]
impl VerifiedContext {
    pub(crate) fn synthetic(identity: Identity, generation: String, official_device: bool) -> Self {
        Self {
            identity,
            generation,
            official_device,
            origin: reqwest::Url::parse("https://openapi.qoder.sh").unwrap(),
            session: None,
        }
    }
}
pub(crate) struct Listing {
    pub identity: Identity,
    pub region: Region,
    pub http: HttpResponse,
}
pub(crate) struct Response {
    pub identity: Identity,
    pub http: HttpResponse,
}
/// Metadata and JSON have one provenance: this constructor consumes the bounded
/// body of the same Response, while its originating runtime is still alive.
pub(crate) struct HttpResponse {
    response: reqwest::Response,
    request_started: Instant,
    body: Option<Value>,
}
impl HttpResponse {
    pub(crate) async fn read(mut response: reqwest::Response, request_started: Instant) -> Self {
        const MAX_BYTES: usize = 1024 * 1024;
        let parsed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            if response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
            {
                return None;
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).ok()
        })
        .await
        .ok()
        .flatten();
        Self {
            response,
            request_started,
            body: parsed,
        }
    }
}
pub(crate) trait CampaignAdapter: Send + Sync {
    fn verify(&self, provider: &Provider, generation: &str) -> Result<VerifiedContext, Dependency>;
    fn list(&self, context: &VerifiedContext) -> Result<Listing, AppError>;
    fn post(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, AppError>;
    /// Synthetic adapters complete immediately under admission. The production
    /// HTTP adapter overrides this to release all guards after the first poll.
    fn post_admitted(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
        observation: &clock::Observation,
        admission: &automatic::Admission<'_>,
    ) -> Result<Response, AppError> {
        admission.dispatch_claim(context, binding, observation, || {
            self.post(context, binding)
        })
    }
    fn get_reward(
        &self,
        context: &VerifiedContext,
        binding: &BoundCampaign,
    ) -> Result<Response, AppError>;
}
fn unavailable() -> AppError {
    AppError::Config("qoder.dependency_unavailable".into())
}

/// Explicit service entry. Source declaration is per-action and generation-bound.
pub(crate) fn prepare_claim(
    db: &Database,
    provider_id: &str,
    target: Option<&Target>,
    declaration: Option<http::PatDeclaration>,
) -> Result<ClaimResult, AppError> {
    prepare_selected(
        db,
        provider_id,
        target,
        &http::HttpAdapter::production(declaration)?,
    )
}

/// Compare the exact configured candidate consumed by the existing SDK route.
/// Neither extraction success nor equality proves PAT family or account subject.
pub(crate) fn credential_candidate(provider: &Provider) -> Result<Option<String>, String> {
    let adapter = crate::proxy::providers::get_adapter(&crate::app_config::AppType::Codex);
    if let Some(config) = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .and_then(crate::grok_config::extract_model_config)
    {
        if config.api_key.is_none() {
            if let Some(env) = config.env_key {
                let mut inline = provider.clone();
                if let Some(object) = inline.settings_config.as_object_mut() {
                    object.remove("config");
                }
                let higher = adapter
                    .as_ref()
                    .and_then(|a| a.extract_auth(&inline))
                    .map(|a| a.api_key);
                return higher.map(Some).ok_or(env);
            }
        }
    }
    Ok(adapter
        .and_then(|a| a.extract_auth(provider))
        .map(|a| a.api_key))
}
pub(crate) fn authentication_changed(before: &Provider, after: &Provider) -> bool {
    let kind = |p: &Provider| p.meta.as_ref().and_then(|m| m.provider_type.clone());
    kind(before) != kind(after) || credential_candidate(before) != credential_candidate(after)
}

#[cfg(test)]
pub(crate) fn prepare_with_adapter(
    db: &Database,
    provider_id: &str,
    target: &Target,
    adapter: &dyn CampaignAdapter,
) -> Result<ClaimResult, AppError> {
    prepare_selected(db, provider_id, Some(target), adapter)
}
pub(crate) fn prepare_selected(
    db: &Database,
    provider_id: &str,
    target: Option<&Target>,
    adapter: &dyn CampaignAdapter,
) -> Result<ClaimResult, AppError> {
    prepare_selected_admitted(db, provider_id, target, adapter, None)
}
fn prepare_selected_admitted(
    db: &Database,
    provider_id: &str,
    target: Option<&Target>,
    adapter: &dyn CampaignAdapter,
    admission: Option<&automatic::Admission<'_>>,
) -> Result<ClaimResult, AppError> {
    if let Some(admission) = admission {
        admission.dispatch(|| Ok(()))?;
    }
    let mut provider = initialize_provider(db, provider_id)?;
    if provider
        .meta
        .as_ref()
        .and_then(|m| m.provider_type.as_deref())
        != Some("qoder_sdk")
    {
        return Err(unavailable());
    }
    if credential_candidate(&provider).is_err() {
        return Ok(ClaimResult::Unavailable(
            Dependency::ExternalCredentialSourceUntracked,
        ));
    }
    let state = provider
        .meta
        .as_ref()
        .and_then(|m| m.qoder_campaign_state.as_ref())
        .ok_or_else(unavailable)?;
    let context = match adapter.verify(&provider, &state.generation) {
        Ok(value) => value,
        Err(reason) => return Ok(ClaimResult::Unavailable(reason)),
    };
    if context.generation != state.generation
        || context.identity.provider_id != provider.id
        || context.identity.uid.trim().is_empty()
        || context.identity.region != Region::International
    {
        return Ok(ClaimResult::Unavailable(Dependency::IdentityMismatch));
    }
    if !context.official_device {
        return Ok(ClaimResult::Unavailable(
            Dependency::OfficialDeviceUnavailable,
        ));
    }
    if admission.is_some_and(|admission| !admission.matches(&context)) {
        return Ok(ClaimResult::Unavailable(Dependency::IdentityMismatch));
    }
    let operations = match db.qoder_operations(&context.identity) {
        Ok(value) => value,
        Err(_) => {
            return Ok(ClaimResult::Unavailable(
                Dependency::ReceiptOwnerUnavailable,
            ))
        }
    };
    if let Some(operation) = operations.iter().find(|o| o.outcome == Outcome::Unknown) {
        return reconcile_operation(db, adapter, &context, operation);
    }
    // A pointer belonging to a different authenticated subject cannot silently be dropped.
    if state.pending.as_ref().is_some_and(|p| {
        !operations
            .iter()
            .any(|o| o.id == p.id && o.generation == p.generation)
    }) {
        return Ok(ClaimResult::Unavailable(Dependency::IdentityMismatch));
    }
    let listing = adapter.list(&context)?;
    if listing.identity != context.identity
        || listing.region != context.identity.region
        || !contract::listing_identity_matches(
            listing.http.body.as_ref().ok_or_else(unavailable)?,
            &context.identity,
        )
    {
        return Ok(ClaimResult::Unavailable(Dependency::IdentityMismatch));
    }
    if listing.http.response.status() != reqwest::StatusCode::OK {
        return Err(AppError::Config("qoder.campaign_unverified".into()));
    }
    // Consume the actual response metadata at the service boundary. An adapter
    // cannot provide a preconstructed trusted clock observation.
    let observation = clock::from_response(
        &listing.http.response,
        &context,
        listing.http.request_started,
    );
    let Some(server_now) = observation.as_ref().and_then(|c| c.for_context(&context)) else {
        return Ok(ClaimResult::Unavailable(Dependency::ClockUnverified));
    };
    db.retire_qoder_operations(&context.identity, server_now)?;
    provider = load(db, provider_id)?;
    let current = provider
        .meta
        .as_ref()
        .and_then(|m| m.qoder_campaign_state.as_ref())
        .ok_or_else(unavailable)?;
    if current.generation != context.generation {
        return Ok(ClaimResult::Unavailable(Dependency::IdentityMismatch));
    }
    let remaining = db.qoder_operations(&context.identity)?;
    if let Some(operation) = remaining.iter().find(|o| o.outcome == Outcome::Unknown) {
        return reconcile_operation(db, adapter, &context, operation);
    }
    // Select the current candidate before consulting historical terminal rows.
    // Old claimed rows may remain in the listing beside the next daily window.
    let selected = if target.is_none() {
        match contract::unique_daily_target(
            &context.identity,
            listing.http.body.as_ref().ok_or_else(unavailable)?,
            server_now,
        ) {
            Ok(value) => value,
            Err(_) => return Ok(ClaimResult::Unavailable(Dependency::NoEligibleCampaign)),
        }
    } else {
        None
    };
    let target = target.or(selected.as_ref());
    if let Some(existing) = remaining.first() {
        if target.is_none_or(|target| existing.binding.target() == *target)
            && contract::listing_matches_binding(
                listing.http.body.as_ref().ok_or_else(unavailable)?,
                &existing.binding,
            )
        {
            return Ok(observed(Action::None, Outcome::AlreadyClaimed, existing));
        }
    }
    let Some(target) = target else {
        return Ok(ClaimResult::Unavailable(Dependency::NoEligibleCampaign));
    };
    let binding = contract::admit(
        &context.identity,
        listing.region,
        target,
        listing.http.body.as_ref().ok_or_else(unavailable)?,
        server_now,
    )
    .map_err(|_| AppError::Config("qoder.campaign_unverified".into()))?;
    if remaining
        .first()
        .is_some_and(|old| binding.start_at() < old.binding.end_at())
    {
        return Err(AppError::Config("qoder.window_not_advanced".into()));
    }
    if !observation
        .as_ref()
        .is_some_and(|clock| clock.window_open(&context, binding.start_at(), binding.end_at()))
    {
        return Ok(ClaimResult::Unavailable(Dependency::CampaignWindowClosed));
    }
    let operation = Operation {
        id: uuid::Uuid::new_v4().to_string(),
        generation: context.generation.clone(),
        binding,
        outcome: Outcome::Unknown,
    };
    // This durable intent is the linearization point. No request may be sent on
    // failure or AlreadyCurrent, and a lost POST result can only enter GET recovery.
    match db.save_qoder_operation(&provider, &operation) {
        Ok(PersistOutcome::Applied) => {}
        Ok(PersistOutcome::AlreadyCurrent) => {
            return Ok(observed(Action::None, Outcome::Unknown, &operation))
        }
        Err(error) => {
            if let Some(existing) = db
                .qoder_operations(&context.identity)?
                .into_iter()
                .find(|o| o.outcome == Outcome::Unknown)
            {
                return reconcile_operation(db, adapter, &context, &existing);
            }
            return Err(error);
        }
    }
    if !observation.as_ref().is_some_and(|clock| {
        clock.window_open(
            &context,
            operation.binding.start_at(),
            operation.binding.end_at(),
        )
    }) {
        // Durable unknown intent is retained. Reopening may only GET, never POST.
        return Ok(ClaimResult::Unavailable(Dependency::CampaignWindowClosed));
    }
    let response = match admission {
        Some(admission) => adapter.post_admitted(
            &context,
            &operation.binding,
            observation.as_ref().ok_or_else(unavailable)?,
            admission,
        ),
        None => adapter.post(&context, &operation.binding),
    };
    complete_response(db, &context, &operation, response, Action::Post)
}
pub(crate) fn initialize_provider(db: &Database, provider_id: &str) -> Result<Provider, AppError> {
    let mut provider = load(db, provider_id)?;
    if !matches!(
        provider
            .meta
            .as_ref()
            .and_then(|m| m.provider_type.as_deref()),
        Some("qoder_sdk" | "qoder_cn")
    ) {
        return Err(unavailable());
    }
    if provider
        .meta
        .as_ref()
        .and_then(|m| m.qoder_campaign_state.as_ref())
        .is_none()
    {
        // The normal owner initializes only non-secret state; no credential is created.
        let vault = db.secrets.read()?;
        let expected = Database::provider_update_digest(&provider)?;
        db.save_provider_with_vault(
            "codex",
            &provider,
            Some(&expected),
            false,
            db.secrets.as_ref(),
            &vault,
        )?;
        drop(vault);
        provider = load(db, provider_id)?;
    }
    Ok(provider)
}
fn load(db: &Database, provider_id: &str) -> Result<Provider, AppError> {
    db.get_provider_by_id(provider_id, "codex")?
        .ok_or_else(|| AppError::Config("qoder.provider_missing".into()))
}
fn observed(action: Action, outcome: Outcome, operation: &Operation) -> ClaimResult {
    ClaimResult::Observed {
        action,
        outcome,
        added_credits: outcome.added_credits(),
        operation: ObservedOperation {
            attempt_id: operation.id.clone(),
            campaign_id: operation.binding.target().campaign_id,
            campaign_key: operation.binding.target().campaign_key,
            start_at: operation.binding.start_at(),
            end_at: operation.binding.end_at(),
        },
    }
}
fn reconcile_operation(
    db: &Database,
    adapter: &dyn CampaignAdapter,
    context: &VerifiedContext,
    operation: &Operation,
) -> Result<ClaimResult, AppError> {
    // read_on rebases only the local Provider reference after matching the same
    // official subject/region. Campaign, window, attempt and generation stay exact.
    let request = contract::Pending {
        binding: operation.binding.clone(),
    };
    contract::reconcile_request(&request, &context.identity).map_err(|_| unavailable())?;
    complete_response(
        db,
        context,
        operation,
        adapter.get_reward(context, &operation.binding),
        Action::Get,
    )
}
fn complete_response(
    db: &Database,
    context: &VerifiedContext,
    operation: &Operation,
    response: Result<Response, AppError>,
    action: Action,
) -> Result<ClaimResult, AppError> {
    let (outcome, server_now) = match response {
        Ok(response) if clock::expected_origin(&response.http.response, context) => {
            let outcome = if action == Action::Get {
                contract::reconcile(
                    &contract::Pending {
                        binding: operation.binding.clone(),
                    },
                    &response.identity,
                    response.http.response.status().as_u16(),
                    response.http.body.as_ref(),
                )
            } else {
                contract::receipt(
                    &operation.binding,
                    &response.identity,
                    response.http.response.status().as_u16(),
                    response.http.body.as_ref(),
                )
            };
            // A different account's Date can never retire this account's evidence.
            let now = (response.identity == context.identity)
                .then(|| {
                    clock::from_response(
                        &response.http.response,
                        context,
                        response.http.request_started,
                    )
                    .and_then(|c| c.for_context(context))
                })
                .flatten();
            (outcome, now)
        }
        Ok(_) | Err(_) => (Outcome::Unknown, None),
    };
    if db
        .finish_qoder_operation(operation, outcome, server_now)
        .is_err()
    {
        return Ok(observed(action, Outcome::Unknown, operation));
    }
    Ok(observed(action, outcome, operation))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimContext {
    pub provider_id: String,
    pub credential_generation: String,
    pub device_dependency: Option<Dependency>,
}
impl crate::services::provider::ProviderService {
    /// Non-secret UI context, along the existing Provider/vault state owner.
    pub fn qoder_daily_claim_context(
        db: &Database,
        provider_id: &str,
    ) -> Result<ClaimContext, AppError> {
        let provider = initialize_provider(db, provider_id)?;
        // CN PAT account management is separate from the unverified CN campaign
        // contract. Do not probe or execute the international device component.
        if provider
            .meta
            .as_ref()
            .and_then(|m| m.provider_type.as_deref())
            != Some("qoder_sdk")
        {
            return Err(unavailable());
        }
        let generation = provider
            .meta
            .as_ref()
            .and_then(|m| m.qoder_campaign_state.as_ref())
            .ok_or_else(unavailable)?
            .generation
            .clone();
        use device::DeviceSource;
        Ok(ClaimContext {
            provider_id: provider.id,
            credential_generation: generation,
            device_dependency: device::InstalledApp.availability().err(),
        })
    }
    /// One explicit user click, with a fresh per-action source declaration. The
    /// only eligible target is selected from the authenticated server listing.
    pub fn prepare_qoder_daily_claim(
        db: &Database,
        provider_id: &str,
        declaration: http::PatDeclaration,
    ) -> Result<Value, AppError> {
        let result = prepare_claim(db, provider_id, None, Some(declaration))?;
        serde_json::to_value(result).map_err(|_| unavailable())
    }
}
