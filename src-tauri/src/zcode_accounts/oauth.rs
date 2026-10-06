//! ZCode login ownership. A waiting IPC call is never the owner of remote work.
use super::core::{AccountSnapshot, OAuthFamily};
use super::official::{BusinessToken, CodingKey, OAuthInit, PersonalProject, PollReady, PollToken};
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

struct Flow {
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

#[derive(Default)]
pub(crate) struct LoginFlowStore(Arc<Mutex<BTreeMap<String, Flow>>>);

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
        if flow.in_flight.is_some() {
            return Err(FlowError::Busy);
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
        if !allowed {
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
    /// Runtime timers call this even if the user never opens another login flow.
    pub(crate) fn expire_due(&self, now: Instant) -> Result<(), FlowError> {
        let mut flows = self.0.lock().map_err(|_| FlowError::Poisoned)?;
        expire_due(&mut flows, now);
        Ok(())
    }
}
impl WorkLease {
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
    pub(crate) fn saved(&self, account_id: String, now: Instant) -> Result<(), FlowError> {
        let mut flows = self.owner.lock().map_err(|_| FlowError::Poisoned)?;
        let flow = flows.get_mut(&self.flow_id).ok_or(FlowError::Missing)?;
        self.matches(flow)?;
        if self.kind != WorkKind::Save {
            return Err(FlowError::WrongStage);
        }
        // The vault writer already committed. Cancellation/expiry cannot erase that fact.
        flow.stage = FlowStage::Saved;
        flow.saved_account_id = Some(account_id);
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
            return flow.in_flight.is_some();
        }
        flow.cancelled.store(true, Ordering::Release);
        flow.draft = None;
        flow.stage = FlowStage::Expired;
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
