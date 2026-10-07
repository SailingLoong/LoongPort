//! ZCode login ownership. A waiting IPC call is never the owner of remote work.
use super::core::{AccountSnapshot, OAuthFamily};
use super::library_context::LibraryContext;
use super::official::{BusinessToken, CodingKey, OAuthInit, PersonalProject, PollReady, PollToken};
use super::transaction::CaptureCommitOutcome;
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const MAX_FLOWS: usize = 2;
const MAX_RECEIPTS: usize = 16;
const FLOW_LIFETIME: Duration = Duration::from_secs(600);
const RECEIPT_LIFETIME: Duration = Duration::from_secs(600);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginError {
    pub code: &'static str,
    pub remedy: &'static str,
    pub committed: bool,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginAccount {
    pub id: String,
    pub label: Option<String>,
    pub duplicate: bool,
    pub identity_source: &'static str,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginConnections {
    pub start: &'static str,
    pub coding: &'static str,
    pub needs_key: bool,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Authorization {
    pub url: String,
    pub expires_at: u64,
    pub poll_interval_sec: u64,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedLogin {
    pub id: String,
    pub outcome: CaptureCommitOutcome,
}
#[derive(Clone, Default)]
pub(crate) struct LoginDisplay {
    pub authorization: Option<Authorization>,
    pub account: Option<LoginAccount>,
    pub connections: Option<LoginConnections>,
    pub project: Option<PersonalProject>,
    pub error: Option<LoginError>,
    pub saved: Option<SavedLogin>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginProgress {
    pub flow_id: String,
    pub phase: &'static str,
    pub family: &'static str,
    pub authorization: Option<Authorization>,
    pub account: Option<LoginAccount>,
    pub connections: Option<LoginConnections>,
    pub project: Option<PersonalProject>,
    pub key_created: bool,
    pub key_may_exist: bool,
    pub key_management_url: &'static str,
    pub error: Option<LoginError>,
    pub saved: Option<SavedLogin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VaultBinding {
    pub root: std::path::PathBuf,
    pub vault_id: String,
    pub key_id: String,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FlowStage {
    Initializing,
    Waiting,
    Preparing,
    KeyRequired,
    Review,
    Saved,
    Cancelled,
    Expired,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkKind {
    Init,
    Poll,
    Prepare,
    CreateKey,
    Save,
    QuerySaved,
    CleanupKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlowError {
    Missing,
    Expired,
    Cancelled,
    Busy,
    WrongStage,
    StaleWork,
    Capacity,
    Poisoned,
}

#[derive(Clone)]
pub(crate) struct LoginDraft {
    pub family: OAuthFamily,
    pub poll_token: PollToken,
    pub init: Option<OAuthInit>,
    pub ready: Option<Arc<PollReady>>,
    pub business: Option<BusinessToken>,
    pub project: Option<PersonalProject>,
    pub coding_key: Option<CodingKey>,
    pub snapshot: Option<AccountSnapshot>,
    pub context: Option<LibraryContext>,
    pub catalog_revision: Option<String>,
    pub save_uncertain: bool,
    pub poll_deadline: Option<Instant>,
    pub app_version: Option<String>,
    pub evidence: Option<super::session_checks::SessionCheckReport>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FlowStatus {
    pub flow_id: String,
    pub stage: FlowStage,
    pub busy: bool,
    pub key_may_exist: bool,
    pub key_created: bool,
    pub saved_account_id: Option<String>,
}

#[derive(Clone)]
pub(crate) enum KeyCleanup {
    Unsubmitted(Arc<super::key_intent::FreshIntent>),
    Copied(super::key_intent::KeyIntent),
}
#[derive(Clone)]
pub(crate) struct SaveQuery {
    pub binding: VaultBinding,
    pub context: LibraryContext,
}
struct Flow {
    family: OAuthFamily,
    display: LoginDisplay,
    save_query: Option<SaveQuery>,
    key_cleanup: Option<KeyCleanup>,
    binding: VaultBinding,
    draft: Option<LoginDraft>,
    stage: FlowStage,
    expires: Instant,
    in_flight: Option<(u64, WorkKind)>,
    next_work: u64,
    cancelled: Arc<AtomicBool>,
    key_may_exist: bool,
    key_created: bool,
    key_intent_generation: Option<u64>,
    saved_account_id: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct LoginFlowStore(Arc<Mutex<BTreeMap<String, Flow>>>);

pub(crate) struct WorkTicket {
    flow_id: String,
    generation: u64,
}
pub(crate) struct WorkLease {
    owner: Arc<Mutex<BTreeMap<String, Flow>>>,
    flow_id: String,
    generation: u64,
    kind: WorkKind,
    cancelled: Arc<AtomicBool>,
}

impl LoginFlowStore {
    pub(crate) fn begin(
        &self,
        binding: VaultBinding,
        family: OAuthFamily,
        poll_token: PollToken,
        now: Instant,
    ) -> Result<String, FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        expire_due(&mut flows, now);
        let active = flows
            .values()
            .filter(|flow| flow.in_flight.is_some() || !terminal(flow.stage))
            .count();
        if active >= MAX_FLOWS || flows.len() >= MAX_FLOWS + MAX_RECEIPTS {
            return Err(FlowError::Capacity);
        }
        let id = uuid::Uuid::new_v4().to_string();
        flows.insert(
            id.clone(),
            Flow {
                family,
                display: LoginDisplay::default(),
                save_query: None,
                key_cleanup: None,
                binding,
                draft: Some(LoginDraft {
                    family,
                    poll_token,
                    init: None,
                    ready: None,
                    business: None,
                    project: None,
                    coding_key: None,
                    snapshot: None,
                    context: None,
                    catalog_revision: None,
                    save_uncertain: false,
                    poll_deadline: None,
                    app_version: None,
                    evidence: None,
                }),
                stage: FlowStage::Initializing,
                expires: now + FLOW_LIFETIME,
                in_flight: None,
                next_work: 0,
                cancelled: Arc::new(AtomicBool::new(false)),
                key_may_exist: false,
                key_created: false,
                key_intent_generation: None,
                saved_account_id: None,
            },
        );
        Ok(id)
    }
    pub(crate) fn acquire(
        &self,
        id: &str,
        kind: WorkKind,
        now: Instant,
    ) -> Result<WorkLease, FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        expire_due(&mut flows, now);
        let flow = flows.get_mut(id).ok_or(FlowError::Missing)?;
        let receipt_query = (kind == WorkKind::QuerySaved && flow.save_query.is_some())
            || (kind == WorkKind::CleanupKey && flow.key_cleanup.is_some());
        if !receipt_query {
            if flow.stage == FlowStage::Expired {
                return Err(FlowError::Expired);
            }
            if terminal(flow.stage) {
                return Err(FlowError::WrongStage);
            }
            if flow.cancelled.load(Ordering::Acquire) {
                return Err(FlowError::Cancelled);
            }
            if now >= flow.expires {
                return Err(FlowError::Expired);
            }
        }
        if flow.in_flight.is_some() {
            return Err(FlowError::Busy);
        }
        if flow.key_cleanup.is_some() && kind != WorkKind::CleanupKey {
            return Err(FlowError::WrongStage);
        }
        let allowed = matches!(
            (flow.stage, kind),
            (FlowStage::Initializing, WorkKind::Init)
                | (FlowStage::Waiting, WorkKind::Poll)
                | (
                    FlowStage::Preparing | FlowStage::KeyRequired | FlowStage::Review,
                    WorkKind::Prepare
                )
                | (FlowStage::KeyRequired, WorkKind::CreateKey)
                | (FlowStage::Review, WorkKind::Save)
        );
        if !allowed && !receipt_query {
            return Err(FlowError::WrongStage);
        }
        flow.next_work = flow.next_work.checked_add(1).ok_or(FlowError::Capacity)?;
        flow.in_flight = Some((flow.next_work, kind));
        Ok(WorkLease {
            owner: Arc::clone(&self.0),
            flow_id: id.into(),
            generation: flow.next_work,
            kind,
            cancelled: Arc::clone(&flow.cancelled),
        })
    }
    pub(crate) fn cancel(&self, id: &str, now: Instant) -> Result<FlowStatus, FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(id).ok_or(FlowError::Missing)?;
        if !terminal(flow.stage) {
            flow.cancelled.store(true, Ordering::Release);
            flow.stage = FlowStage::Cancelled;
            flow.expires = now + RECEIPT_LIFETIME;
            flow.draft = None;
            flow.display.authorization = None;
            if flow.save_query.is_some() {
                flow.display.error = Some(LoginError {
                    code: "zcode.account.save_result_unknown",
                    remedy: "queryOriginal",
                    committed: false,
                });
            }
        }
        Ok(describe(id, flow))
    }
    pub(crate) fn status(&self, id: &str, now: Instant) -> Result<FlowStatus, FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        expire_due(&mut flows, now);
        let flow = flows.get(id).ok_or(FlowError::Missing)?;
        if now >= flow.expires && flow.in_flight.is_none() {
            return Err(FlowError::Expired);
        }
        Ok(describe(id, flow))
    }
    pub(crate) fn progress(&self, id: &str, now: Instant) -> Result<LoginProgress, FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        expire_due(&mut flows, now);
        let flow = flows.get(id).ok_or(FlowError::Missing)?;
        Ok(LoginProgress {
            flow_id: id.into(),
            phase: match flow.stage {
                FlowStage::Initializing | FlowStage::Preparing => "preparing",
                FlowStage::Waiting => "waiting",
                FlowStage::KeyRequired => "keyRequired",
                FlowStage::Review => "review",
                FlowStage::Saved => "saved",
                FlowStage::Cancelled => "cancelled",
                FlowStage::Expired => "expired",
                FlowStage::Failed => "failed",
            },
            family: match flow.family {
                OAuthFamily::BigModel => "bigmodel",
                OAuthFamily::Zai => "zai",
            },
            authorization: flow.display.authorization.clone(),
            account: flow.display.account.clone(),
            connections: flow.display.connections.clone(),
            project: flow.display.project.clone(),
            key_created: flow.key_created,
            key_may_exist: flow.key_may_exist,
            key_management_url: match flow.family {
                OAuthFamily::BigModel => "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
                OAuthFamily::Zai => "https://z.ai/manage-apikey/apikey-list",
            },
            error: flow.display.error.clone(),
            saved: flow.display.saved.clone(),
        })
    }
    pub(crate) fn fail_abandoned(&self, ticket: &WorkTicket) -> Result<(), FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&ticket.flow_id).ok_or(FlowError::Missing)?;
        if flow.next_work == ticket.generation && flow.in_flight.is_none() && !terminal(flow.stage)
        {
            flow.stage = FlowStage::Failed;
            flow.draft = None;
            flow.display.authorization = None;
            flow.display.error = Some(LoginError {
                code: "zcode.account.operation_failed",
                remedy: "queryOriginal",
                committed: false,
            });
        }
        Ok(())
    }
    pub(crate) fn recovery_kind(&self, id: &str) -> Result<Option<WorkKind>, FlowError> {
        let flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get(id).ok_or(FlowError::Missing)?;
        if flow.in_flight.is_some() {
            return Ok(None);
        }
        if flow.save_query.is_some() {
            return Ok(Some(WorkKind::QuerySaved));
        }
        if flow.key_cleanup.is_some() {
            return Ok(Some(WorkKind::CleanupKey));
        }
        if terminal(flow.stage) {
            return Ok(None);
        }
        if flow.stage == FlowStage::Review
            && flow
                .draft
                .as_ref()
                .is_some_and(|draft| draft.save_uncertain)
        {
            return Ok(Some(WorkKind::Save));
        }
        if matches!(flow.stage, FlowStage::KeyRequired | FlowStage::Review)
            && flow
                .display
                .error
                .as_ref()
                .is_some_and(|error| error.remedy == "queryOriginal")
        {
            return Ok(Some(WorkKind::Prepare));
        }
        Ok(None)
    }
    /// Runtime timers call this even if the user never opens another login flow.
    pub(crate) fn expire_due(&self, now: Instant) -> Result<(), FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        expire_due(&mut flows, now);
        Ok(())
    }
}
impl WorkLease {
    /// These proofs contain only exact local operation identity, never tokens.
    /// They outlive a cancelled secret draft solely to finish local cleanup.
    pub(crate) fn record_key_cleanup(&self, proof: KeyCleanup) -> Result<(), FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if !matches!(self.kind, WorkKind::CreateKey | WorkKind::Prepare) {
            return Err(FlowError::WrongStage);
        }
        flow.key_cleanup = Some(proof);
        Ok(())
    }
    pub(crate) fn key_cleanup_query(&self) -> Result<(VaultBinding, KeyCleanup), FlowError> {
        let flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        Ok((
            flow.binding.clone(),
            flow.key_cleanup.clone().ok_or(FlowError::WrongStage)?,
        ))
    }
    pub(crate) fn key_cleanup_failed(&self) -> Result<(), FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if flow.key_cleanup.is_none() {
            return Err(FlowError::WrongStage);
        }
        flow.display.error = Some(LoginError {
            code: "zcode.account.key_cleanup_pending",
            remedy: "queryOriginal",
            committed: false,
        });
        if !terminal(flow.stage) {
            flow.stage = FlowStage::KeyRequired;
        }
        flow.in_flight = None;
        Ok(())
    }
    pub(crate) fn key_cleanup_finished(&self) -> Result<bool, FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        match flow.key_cleanup.take() {
            Some(KeyCleanup::Copied(_)) => {
                flow.key_may_exist = false;
                flow.key_intent_generation = None;
            }
            Some(KeyCleanup::Unsubmitted(_)) if !flow.key_created => {
                flow.key_may_exist = false;
                flow.key_intent_generation = None;
            }
            _ => {}
        }
        flow.display.error = None;
        if terminal(flow.stage) {
            flow.in_flight = None;
            return Ok(false);
        }
        Ok(true)
    }
    pub(crate) fn mark_save_started(&self) -> Result<(), FlowError> {
        self.check()?;
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if self.kind != WorkKind::Save || flow.cancelled.load(Ordering::Acquire) {
            return Err(FlowError::Cancelled);
        }
        let draft = flow.draft.as_mut().ok_or(FlowError::WrongStage)?;
        flow.save_query = Some(SaveQuery {
            binding: flow.binding.clone(),
            context: draft.context.clone().ok_or(FlowError::WrongStage)?,
        });
        draft.save_uncertain = true;
        Ok(())
    }
    pub(crate) fn save_query(&self) -> Result<SaveQuery, FlowError> {
        let flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if !matches!(self.kind, WorkKind::Save | WorkKind::QuerySaved) {
            return Err(FlowError::WrongStage);
        }
        flow.save_query.clone().ok_or(FlowError::WrongStage)
    }
    pub(crate) fn save_query_unavailable(&self) -> Result<(), FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if flow.save_query.is_none() {
            return Err(FlowError::WrongStage);
        }
        flow.display.error = Some(LoginError {
            code: "zcode.account.save_result_unknown",
            remedy: "queryOriginal",
            committed: false,
        });
        if !terminal(flow.stage) {
            flow.stage = FlowStage::Review;
        }
        flow.in_flight = None;
        Ok(())
    }
    /// The owned writer has finished and an authenticated catalog has no receipt.
    /// A cancelled/expired operation needs no credentials to record that fact.
    pub(crate) fn save_not_committed(&self) -> Result<bool, FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        flow.save_query = None;
        if let Some(draft) = flow.draft.as_mut() {
            draft.save_uncertain = false;
        }
        if terminal(flow.stage) {
            flow.in_flight = None;
            flow.display.error = None;
            return Ok(false);
        }
        Ok(true)
    }
    pub(crate) fn ticket(&self) -> WorkTicket {
        WorkTicket {
            flow_id: self.flow_id.clone(),
            generation: self.generation,
        }
    }
    pub(crate) fn flow_id(&self) -> &str {
        &self.flow_id
    }
    fn matches(&self, flow: &Flow) -> Result<(), FlowError> {
        if flow.in_flight == Some((self.generation, self.kind)) {
            Ok(())
        } else {
            Err(FlowError::StaleWork)
        }
    }
    pub(crate) fn check(&self) -> Result<(), FlowError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(FlowError::Cancelled);
        }
        let flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if Instant::now() >= flow.expires {
            return Err(FlowError::Expired);
        }
        Ok(())
    }
    pub(crate) fn draft(&self) -> Result<(VaultBinding, LoginDraft), FlowError> {
        self.check()?;
        let flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if flow.cancelled.load(Ordering::Acquire) {
            return Err(FlowError::Cancelled);
        }
        Ok((
            flow.binding.clone(),
            flow.draft.clone().ok_or(FlowError::StaleWork)?,
        ))
    }
    pub(crate) fn update(
        &self,
        change: impl FnOnce(&mut LoginDraft, &mut LoginDisplay),
    ) -> Result<(), FlowError> {
        self.check()?;
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if flow.cancelled.load(Ordering::Acquire) {
            return Err(FlowError::Cancelled);
        }
        change(
            flow.draft.as_mut().ok_or(FlowError::StaleWork)?,
            &mut flow.display,
        );
        Ok(())
    }
    pub(crate) fn bound_deadline(&self, deadline: Instant) -> Result<(), FlowError> {
        self.check()?;
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        flow.expires = flow.expires.min(deadline);
        Ok(())
    }
    pub(crate) fn recover_key_intent(&self, created: bool) -> Result<(), FlowError> {
        self.check()?;
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        flow.key_may_exist = true;
        flow.key_created |= created;
        Ok(())
    }
    pub(crate) fn finish(
        &self,
        stage: FlowStage,
        update: impl FnOnce(&mut LoginDraft),
    ) -> Result<(), FlowError> {
        if stage == FlowStage::Saved {
            return Err(FlowError::WrongStage);
        }
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if flow.cancelled.load(Ordering::Acquire) {
            return Err(FlowError::Cancelled);
        }
        if Instant::now() >= flow.expires {
            return Err(FlowError::Expired);
        }
        update(flow.draft.as_mut().ok_or(FlowError::StaleWork)?);
        flow.stage = stage;
        flow.in_flight = None;
        if terminal(stage) {
            flow.display.authorization = None;
            flow.draft = None;
        }
        Ok(())
    }
    pub(crate) fn mark_key_intent(&self) -> Result<(), FlowError> {
        self.check()?;
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if flow.cancelled.load(Ordering::Acquire) {
            return Err(FlowError::Cancelled);
        }
        if self.kind != WorkKind::CreateKey || flow.key_may_exist {
            return Err(FlowError::WrongStage);
        }
        flow.key_may_exist = true;
        flow.key_cleanup = None;
        flow.key_intent_generation = Some(self.generation);
        Ok(())
    }
    pub(crate) fn mark_key_created(&self) -> Result<(), FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if self.kind != WorkKind::CreateKey {
            return Err(FlowError::WrongStage);
        }
        flow.key_may_exist = true;
        flow.key_created = true;
        Ok(())
    }
    pub(crate) fn mark_key_not_sent(&self) -> Result<(), FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if self.kind != WorkKind::CreateKey
            || flow.key_created
            || flow.key_intent_generation != Some(self.generation)
        {
            return Err(FlowError::WrongStage);
        }
        flow.key_may_exist = false;
        flow.key_intent_generation = None;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn saved(&self, account_id: String, now: Instant) -> Result<(), FlowError> {
        self.saved_with_outcome(account_id, CaptureCommitOutcome::Saved, now)
    }
    pub(crate) fn saved_with_outcome(
        &self,
        account_id: String,
        outcome: CaptureCommitOutcome,
        now: Instant,
    ) -> Result<(), FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if !matches!(self.kind, WorkKind::Save | WorkKind::QuerySaved) {
            return Err(FlowError::WrongStage);
        }
        // The vault writer already committed. Cancellation/expiry cannot erase that fact.
        flow.stage = FlowStage::Saved;
        flow.saved_account_id = Some(account_id.clone());
        flow.save_query = None;
        flow.display.saved = Some(SavedLogin {
            id: account_id,
            outcome,
        });
        flow.display.authorization = None;
        flow.display.error = None;
        flow.expires = now + RECEIPT_LIFETIME;
        flow.in_flight = None;
        flow.draft = None;
        Ok(())
    }
}
impl Drop for WorkLease {
    fn drop(&mut self) {
        if let Ok(mut flows) = self.owner.lock() {
            if let Some(flow) = flows.get_mut(&self.flow_id) {
                // Flow ID alone is insufficient: a successor poll may already own it.
                if flow.in_flight == Some((self.generation, self.kind)) {
                    flow.in_flight = None;
                    if flow.cancelled.load(Ordering::Acquire) {
                        flow.draft = None;
                    }
                }
            }
        }
    }
}

fn terminal(stage: FlowStage) -> bool {
    matches!(
        stage,
        FlowStage::Saved | FlowStage::Cancelled | FlowStage::Expired | FlowStage::Failed
    )
}
fn expire_due(flows: &mut BTreeMap<String, Flow>, now: Instant) {
    flows.retain(|_, flow| {
        if now < flow.expires {
            return true;
        }
        if terminal(flow.stage) {
            return flow.in_flight.is_some()
                || flow.save_query.is_some()
                || flow.key_cleanup.is_some();
        }
        flow.cancelled.store(true, Ordering::Release);
        flow.draft = None;
        flow.stage = FlowStage::Expired;
        flow.display.authorization = None;
        flow.expires = now + RECEIPT_LIFETIME;
        true
    });
}
fn describe(id: &str, flow: &Flow) -> FlowStatus {
    FlowStatus {
        flow_id: id.into(),
        stage: flow.stage,
        busy: flow.in_flight.is_some(),
        key_may_exist: flow.key_may_exist,
        key_created: flow.key_created,
        saved_account_id: flow.saved_account_id.clone(),
    }
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
