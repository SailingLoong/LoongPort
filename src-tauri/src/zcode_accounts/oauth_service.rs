//! Login orchestration over the existing flow and vault owners. No native writes.
use super::core::AccountSnapshot;
#[cfg(test)]
use super::core::OAuthFamily;
use super::key_intent::{FreshIntent, KeyIntent, KeyScope, Reservation};
use super::library_context::LibraryContext;
use super::oauth::*;
use super::official::*;
use super::transaction::CaptureCommitOutcome;
use std::{future::Future, pin::Pin, sync::Arc, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreFailure {
    Changed,
    Unavailable,
    Unknown,
    Capacity,
}
pub(crate) type StoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, StoreFailure>> + Send + 'a>>;
pub(crate) struct CatalogView {
    pub revision: String,
    pub duplicate: bool,
}
pub(crate) trait LoginPersistence: Send + Sync {
    fn check(&self, binding: &VaultBinding) -> Result<(), StoreFailure>;
    fn catalog<'a>(
        &'a self,
        binding: &'a VaultBinding,
        context: &'a LibraryContext,
        snapshot: &'a AccountSnapshot,
    ) -> StoreFuture<'a, CatalogView>;
    fn reserve<'a>(
        &'a self,
        binding: &'a VaultBinding,
        scope: KeyScope,
    ) -> StoreFuture<'a, Reservation>;
    fn intent<'a>(
        &'a self,
        binding: &'a VaultBinding,
        scope: &'a KeyScope,
    ) -> StoreFuture<'a, Option<KeyIntent>>;
    fn mark_created<'a>(
        &'a self,
        binding: &'a VaultBinding,
        grant: Arc<FreshIntent>,
    ) -> StoreFuture<'a, ()>;
    fn clear_unsubmitted<'a>(
        &'a self,
        binding: &'a VaultBinding,
        grant: Arc<FreshIntent>,
    ) -> StoreFuture<'a, ()>;
    fn clear_resolved<'a>(
        &'a self,
        binding: &'a VaultBinding,
        intent: &'a KeyIntent,
    ) -> StoreFuture<'a, ()>;
    fn saved_receipt<'a>(
        &'a self,
        binding: &'a VaultBinding,
        context: &'a LibraryContext,
        request_id: &'a str,
    ) -> StoreFuture<'a, Option<super::checkpoint::LoginReceipt>>;
    fn save<'a>(
        &'a self,
        binding: &'a VaultBinding,
        context: &'a LibraryContext,
        revision: &'a str,
        request_id: &'a str,
        snapshot: &'a AccountSnapshot,
        evidence: Option<&'a super::session_checks::SessionCheckReport>,
        update_duplicate: bool,
    ) -> StoreFuture<'a, CaptureCommitOutcome>;
}

fn retryable(error: OfficialError) -> bool {
    matches!(
        error,
        OfficialError::Timeout
            | OfficialError::Transport
            | OfficialError::Http(408 | 425 | 429 | 500..=599)
    )
}
pub(crate) struct LoginService<T, P> {
    pub flows: LoginFlowStore,
    client: OfficialClient<T>,
    persistence: P,
}
impl<T: OfficialTransport, P: LoginPersistence> LoginService<T, P> {
    pub(crate) fn new(transport: T, persistence: P) -> Self {
        Self {
            flows: LoginFlowStore::default(),
            client: OfficialClient::new(transport),
            persistence,
        }
    }
    pub(crate) async fn initialize(
        &self,
        lease: WorkLease,
        context: LibraryContext,
        app_version: Option<String>,
    ) -> Result<(), FlowError> {
        let (binding, draft) = lease.draft()?;
        lease.update(|draft, _| {
            draft.context = Some(context);
            draft.app_version = app_version;
        })?;
        let check = || self.check(&lease, &binding);
        match self
            .client
            .init(draft.family, &draft.poll_token, &check)
            .await
        {
            Ok(init) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| FlowError::Expired)?
                    .as_secs();
                let remaining = init.expires_at.saturating_sub(now).min(300);
                if remaining == 0 {
                    return lease.finish(FlowStage::Expired, |_| {});
                }
                lease.update(|draft, display| {
                    draft.poll_deadline =
                        Some(Instant::now() + std::time::Duration::from_secs(remaining));
                    display.authorization = Some(Authorization {
                        url: init.authorize_url.clone(),
                        expires_at: init.expires_at,
                        poll_interval_sec: init.poll_interval_sec,
                    });
                })?;
                lease.finish(FlowStage::Waiting, |draft| draft.init = Some(init))
            }
            Err(_) => self.error(
                &lease,
                FlowStage::Failed,
                "zcode.account.official_unavailable",
                "queryOriginal",
            ),
        }
    }
    /// Returns true only when this poll committed one complete ready response.
    pub(crate) async fn poll(&self, lease: WorkLease) -> Result<bool, FlowError> {
        let (binding, draft) = lease.draft()?;
        if draft
            .poll_deadline
            .is_none_or(|deadline| Instant::now() >= deadline)
        {
            lease.finish(FlowStage::Expired, |_| {})?;
            return Ok(false);
        }
        let init = draft.init.as_ref().ok_or(FlowError::WrongStage)?;
        let check = || self.check(&lease, &binding);
        match self
            .client
            .poll(draft.family, &init.flow_id, &draft.poll_token, &check)
            .await
        {
            Ok(OAuthPoll::Pending) => {
                lease.update(|_, display| display.error = None)?;
                lease.finish(FlowStage::Waiting, |_| {})?;
                Ok(false)
            }
            Ok(OAuthPoll::Ready(ready)) => {
                lease.update(|_, display| {
                    display.authorization = None;
                    display.error = None;
                })?;
                lease.finish(FlowStage::Preparing, |draft| {
                    draft.ready = Some(Arc::new(ready))
                })?;
                Ok(true)
            }
            Err(error) if retryable(error) => {
                self.error(
                    &lease,
                    FlowStage::Waiting,
                    "zcode.account.official_unavailable",
                    "queryOriginal",
                )?;
                Ok(false)
            }
            _ => {
                self.error(
                    &lease,
                    FlowStage::Failed,
                    "zcode.account.official_unavailable",
                    "queryOriginal",
                )?;
                Ok(false)
            }
        }
    }
    pub(crate) async fn prepare(&self, lease: WorkLease) -> Result<(), FlowError> {
        let (binding, draft) = lease.draft()?;
        let ready = draft.ready.as_ref().ok_or(FlowError::WrongStage)?;
        let check = || self.check(&lease, &binding);
        let business = match draft.business {
            Some(business) => business,
            None => match self
                .client
                .normalize_business_token(draft.family, &ready.provider_access_token, &check)
                .await
            {
                Ok(business) => business,
                Err(error) => {
                    return self.error(
                        &lease,
                        if retryable(error) {
                            FlowStage::Preparing
                        } else {
                            FlowStage::Failed
                        },
                        "zcode.account.official_unavailable",
                        "queryOriginal",
                    )
                }
            },
        };
        lease.update(|draft, _| draft.business = Some(business.clone()))?;
        let customer = self.client.read_customer(&business, &check).await;
        lease.check()?;
        let project = customer.ok().and_then(|customer| customer.personal_project);
        lease.update(|draft, display| {
            draft.project = project.clone();
            display.project = project.clone();
        })?;
        if let Some(project) = &project {
            let scope = KeyScope::new(
                draft.family,
                &ready.user.id,
                &project.organization_id,
                &project.project_id,
            )
            .map_err(|_| FlowError::WrongStage)?;
            match self.persistence.intent(&binding, &scope).await {
                Ok(Some(intent)) => {
                    let (_, current) = lease.draft()?;
                    return self.recover_key(&lease, &binding, &current, &intent).await;
                }
                Err(_) => {
                    return self.error(
                        &lease,
                        FlowStage::KeyRequired,
                        "zcode.account.key_cleanup_pending",
                        "queryOriginal",
                    )
                }
                Ok(None) => {}
            }
        }
        match project {
            Some(project) => match self.client.discover_key(&business, &project, &check).await {
                Ok(Some(key)) => self.candidate(&lease, Some(key), FlowStage::Review).await,
                Ok(None) => self.candidate(&lease, None, FlowStage::KeyRequired).await,
                Err(_) => self.candidate(&lease, None, FlowStage::Review).await,
            },
            None => self.candidate(&lease, None, FlowStage::Review).await,
        }
    }
    pub(crate) async fn decline_key(&self, lease: WorkLease) -> Result<(), FlowError> {
        let (_, draft) = lease.draft()?;
        self.candidate(&lease, draft.coding_key, FlowStage::Review)
            .await
    }
    /// An explicit original-result read can only inspect prior writes and Key lists.
    pub(crate) async fn recover(&self, lease: WorkLease) -> Result<(), FlowError> {
        if lease.save_query().is_ok() {
            return self.recover_save(&lease).await;
        }
        if lease.key_cleanup_query().is_ok() {
            if !self.clean_key(&lease).await? {
                return Ok(());
            }
            let (_, draft) = lease.draft()?;
            return self
                .candidate(&lease, draft.coding_key, FlowStage::Review)
                .await;
        }

        let (binding, draft) = lease.draft()?;
        if let (Some(ready), Some(project)) = (&draft.ready, &draft.project) {
            let scope = KeyScope::new(
                draft.family,
                &ready.user.id,
                &project.organization_id,
                &project.project_id,
            )
            .map_err(|_| FlowError::WrongStage)?;
            match self.persistence.intent(&binding, &scope).await {
                Ok(Some(intent)) => {
                    return self.recover_key(&lease, &binding, &draft, &intent).await
                }
                Ok(None) => {}
                Err(_) => {
                    let stage = self.flows.status(lease.flow_id(), Instant::now())?.stage;
                    return self.error(
                        &lease,
                        stage,
                        "zcode.account.key_cleanup_pending",
                        "queryOriginal",
                    );
                }
            }
        }
        self.candidate(&lease, draft.coding_key, FlowStage::Review)
            .await
    }
    fn check(&self, lease: &WorkLease, binding: &VaultBinding) -> Result<(), OfficialError> {
        lease.check().map_err(|_| OfficialError::Cancelled)?;
        self.persistence
            .check(binding)
            .map_err(|_| OfficialError::Cancelled)
    }
    fn error(
        &self,
        lease: &WorkLease,
        stage: FlowStage,
        code: &'static str,
        remedy: &'static str,
    ) -> Result<(), FlowError> {
        lease.update(|_, display| {
            display.error = Some(LoginError {
                code,
                remedy,
                committed: false,
            })
        })?;
        lease.finish(stage, |_| {})
    }
    async fn candidate(
        &self,
        lease: &WorkLease,
        key: Option<CodingKey>,
        stage: FlowStage,
    ) -> Result<(), FlowError> {
        let (binding, draft) = lease.draft()?;
        let ready = draft.ready.ok_or(FlowError::WrongStage)?;
        let context = draft.context.ok_or(FlowError::WrongStage)?;
        let business = draft.business.ok_or(FlowError::WrongStage)?;
        let native = context.cipher().map_err(|_| FlowError::WrongStage)?;
        let snapshot =
            super::oauth_account::build_snapshot(&native, &ready, &business, key.as_ref())
                .map_err(|_| FlowError::WrongStage)?;
        // Keep recovered credentials in this owned draft if a later catalog read fails.
        lease.update(|draft, _| {
            draft.coding_key = key;
            draft.snapshot = Some(snapshot.clone());
        })?;
        let check = || self.check(lease, &binding);
        let checked_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| FlowError::Expired)?
            .as_secs();
        let evidence = super::session_checks::check_session(
            &self.client,
            &native,
            &snapshot,
            draft.app_version.as_deref(),
            checked_at,
            &check,
        )
        .await
        .map_err(|_| FlowError::Cancelled)?;
        let display = evidence.display();
        let connection = |state, entitlement| {
            use super::session_checks::{CheckState, EntitlementState};
            if state == CheckState::Accepted && entitlement == EntitlementState::Available {
                "ready"
            } else if state == CheckState::Unavailable
                || entitlement == EntitlementState::Unavailable
            {
                "unavailable"
            } else {
                "unknown"
            }
        };
        let start = connection(display.start.check.state, display.start.entitlement);
        let coding = connection(display.coding.check.state, display.coding.entitlement);
        lease.update(|draft, _| draft.evidence = Some(evidence))?;
        let catalog = match self
            .persistence
            .catalog(&binding, &context, &snapshot)
            .await
        {
            Ok(catalog) => catalog,
            Err(_) => {
                return self.error(
                    lease,
                    FlowStage::Review,
                    "zcode.account.storage_failed",
                    "queryOriginal",
                )
            }
        };
        lease.update(|draft, display| {
            draft.catalog_revision = Some(catalog.revision);
            display.account = Some(LoginAccount {
                id: snapshot.identity().opaque_id(),
                label: native.profile_label(&snapshot).ok().flatten(),
                duplicate: catalog.duplicate,
                identity_source: "officialLogin",
            });
            display.connections = Some(LoginConnections {
                start,
                coding,
                needs_key: draft.coding_key.is_none(),
            });
            display.error = None;
        })?;
        lease.finish(stage, |_| {})
    }
    async fn clean_key(&self, lease: &WorkLease) -> Result<bool, FlowError> {
        let (binding, proof) = lease.key_cleanup_query()?;
        let result = match proof {
            KeyCleanup::Unsubmitted(grant) => {
                self.persistence.clear_unsubmitted(&binding, grant).await
            }
            KeyCleanup::Copied(intent) => self.persistence.clear_resolved(&binding, &intent).await,
        };
        if result.is_err() {
            lease.key_cleanup_failed()?;
            return Ok(false);
        }
        lease.key_cleanup_finished()
    }
    async fn copied_key(
        &self,
        lease: &WorkLease,
        key: CodingKey,
        intent: &KeyIntent,
    ) -> Result<(), FlowError> {
        lease.record_key_cleanup(KeyCleanup::Copied(intent.clone()))?;
        let candidate = lease.update(|draft, _| draft.coding_key = Some(key.clone()));
        if !self.clean_key(lease).await? {
            return Ok(());
        }
        candidate?;
        self.candidate(lease, Some(key), FlowStage::Review).await
    }
    async fn recover_key(
        &self,
        lease: &WorkLease,
        binding: &VaultBinding,
        draft: &LoginDraft,
        intent: &KeyIntent,
    ) -> Result<(), FlowError> {
        lease.recover_key_intent(intent.state == super::key_intent::IntentState::Created)?;
        let business = draft.business.as_ref().ok_or(FlowError::WrongStage)?;
        let project = draft.project.as_ref().ok_or(FlowError::WrongStage)?;
        let check = || self.check(lease, binding);
        match self.client.discover_key(business, project, &check).await {
            Ok(Some(key)) => self.copied_key(lease, key, intent).await,
            _ => self.error(
                lease,
                FlowStage::KeyRequired,
                "zcode.account.key_result_unknown",
                "queryOriginal",
            ),
        }
    }
    pub(crate) async fn create_key(
        &self,
        lease: WorkLease,
        organization: &str,
        project: &str,
    ) -> Result<(), FlowError> {
        let (binding, draft) = lease.draft()?;
        let ready = draft.ready.as_ref().ok_or(FlowError::WrongStage)?;
        let selected = draft.project.as_ref().ok_or(FlowError::WrongStage)?;
        let business = draft.business.as_ref().ok_or(FlowError::WrongStage)?;
        if selected.organization_id != organization || selected.project_id != project {
            return self.error(
                &lease,
                FlowStage::KeyRequired,
                "zcode.account.source_changed",
                "retryKeyConsent",
            );
        }
        self.check(&lease, &binding)
            .map_err(|_| FlowError::Cancelled)?;
        let scope = KeyScope::new(draft.family, &ready.user.id, organization, project)
            .map_err(|_| FlowError::WrongStage)?;
        let reservation = match self.persistence.reserve(&binding, scope).await {
            Ok(value) => value,
            Err(_) => {
                return self.error(
                    &lease,
                    FlowStage::KeyRequired,
                    "zcode.account.storage_failed",
                    "queryOriginal",
                )
            }
        };
        let grant = match reservation {
            Reservation::Existing(intent) => {
                return self.recover_key(&lease, &binding, &draft, &intent).await
            }
            Reservation::Fresh(grant) => Arc::new(grant),
        };
        lease.record_key_cleanup(KeyCleanup::Unsubmitted(Arc::clone(&grant)))?;
        let check = || self.check(&lease, &binding);
        match self.client.discover_key(business, selected, &check).await {
            Ok(Some(key)) => {
                let candidate = lease.update(|draft, _| draft.coding_key = Some(key.clone()));
                if !self.clean_key(&lease).await? {
                    return Ok(());
                }
                candidate?;
                return self.candidate(&lease, Some(key), FlowStage::Review).await;
            }
            Err(_) => {
                if !self.clean_key(&lease).await? {
                    return Ok(());
                }
                return self.error(
                    &lease,
                    FlowStage::KeyRequired,
                    "zcode.account.request_not_sent",
                    "retryKeyConsent",
                );
            }
            Ok(None) => {}
        }
        if let Err(error) = lease.mark_key_intent() {
            let _ = self.clean_key(&lease).await?;
            return Err(error);
        }
        let permit = KeyCreationPermit::new(&ready.user.id, draft.family, selected.clone())
            .map_err(|_| FlowError::WrongStage)?;
        let check = || self.check(&lease, &binding);
        match self
            .client
            .create_key_once(business, &ready.user.id, permit, &check)
            .await
        {
            CreateKeyOutcome::NotSent(_) => {
                lease.record_key_cleanup(KeyCleanup::Unsubmitted(Arc::clone(&grant)))?;
                if !self.clean_key(&lease).await? {
                    return Ok(());
                }
                self.error(
                    &lease,
                    FlowStage::KeyRequired,
                    "zcode.account.request_not_sent",
                    "retryKeyConsent",
                )
            }
            CreateKeyOutcome::MayHaveBeenSent(_) => self.error(
                &lease,
                FlowStage::KeyRequired,
                "zcode.account.key_result_unknown",
                "queryOriginal",
            ),
            CreateKeyOutcome::Created(summary) => {
                lease.mark_key_created()?;
                let _ = self
                    .persistence
                    .mark_created(&binding, Arc::clone(&grant))
                    .await;
                match self
                    .client
                    .copy_key(business, selected, &summary, &check)
                    .await
                {
                    Ok(key) => self.copied_key(&lease, key, &grant.receipt()).await,
                    Err(_) => self.error(
                        &lease,
                        FlowStage::KeyRequired,
                        "zcode.account.key_result_unknown",
                        "queryOriginal",
                    ),
                }
            }
        }
    }
    pub(crate) async fn save(
        &self,
        lease: WorkLease,
        update_duplicate: bool,
    ) -> Result<(), FlowError> {
        if lease.save_query().is_ok() {
            return self.recover_save(&lease).await;
        }
        let (binding, draft) = lease.draft()?;
        let context = draft.context.as_ref().ok_or(FlowError::WrongStage)?;
        let snapshot = draft.snapshot.as_ref().ok_or(FlowError::WrongStage)?;
        let revision = draft
            .catalog_revision
            .as_deref()
            .ok_or(FlowError::WrongStage)?;
        self.check(&lease, &binding)
            .map_err(|_| FlowError::Cancelled)?;
        lease.mark_save_started()?;
        match self
            .persistence
            .save(
                &binding,
                context,
                revision,
                lease.flow_id(),
                snapshot,
                draft.evidence.as_ref(),
                update_duplicate,
            )
            .await
        {
            Ok(outcome) => {
                lease.saved_with_outcome(snapshot.identity().opaque_id(), outcome, Instant::now())
            }
            Err(StoreFailure::Capacity) => {
                if lease.save_not_committed()? {
                    self.error(
                        &lease,
                        FlowStage::Review,
                        "zcode.account.resource_limit",
                        "reviewSavedData",
                    )
                } else {
                    Ok(())
                }
            }
            Err(_) => self.recover_save(&lease).await,
        }
    }
    async fn recover_save(&self, lease: &WorkLease) -> Result<(), FlowError> {
        let query = lease.save_query()?;
        match self
            .persistence
            .saved_receipt(&query.binding, &query.context, lease.flow_id())
            .await
        {
            Ok(Some(receipt)) => {
                lease.saved_with_outcome(receipt.account_id, receipt.outcome, Instant::now())
            }
            Ok(None) => {
                let Ok((binding, draft)) = lease.draft() else {
                    lease.save_not_committed()?;
                    return Ok(());
                };
                let snapshot = draft.snapshot.as_ref().ok_or(FlowError::WrongStage)?;
                let catalog = match self
                    .persistence
                    .catalog(&binding, &query.context, snapshot)
                    .await
                {
                    Ok(catalog) => catalog,
                    Err(_) => return lease.save_query_unavailable(),
                };
                let changed = draft.catalog_revision.as_ref() != Some(&catalog.revision);
                let native = query.context.cipher().map_err(|_| FlowError::WrongStage)?;
                if lease
                    .update(|draft, display| {
                        draft.catalog_revision = Some(catalog.revision);
                        display.account = Some(LoginAccount {
                            id: snapshot.identity().opaque_id(),
                            label: native.profile_label(snapshot).ok().flatten(),
                            duplicate: catalog.duplicate,
                            identity_source: "officialLogin",
                        });
                    })
                    .is_err()
                {
                    lease.save_not_committed()?;
                    return Ok(());
                }
                if !lease.save_not_committed()? {
                    return Ok(());
                }
                self.error(
                    lease,
                    FlowStage::Review,
                    if changed {
                        "zcode.account.catalog_changed"
                    } else {
                        "zcode.account.storage_failed"
                    },
                    "retrySave",
                )
            }
            Err(_) => lease.save_query_unavailable(),
        }
    }
}

#[cfg(test)]
#[path = "oauth_service_tests.rs"]
mod tests;
