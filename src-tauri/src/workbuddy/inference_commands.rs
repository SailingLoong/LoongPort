//! Explicit account-row actions. This bounded five-minute map holds only opaque
//! pending login flows, never a second account/token store. Grants stay in the
//! original encrypted WorkBuddy owner; renderer DTOs contain no credentials.
use super::{
    inference::{
        self, AuthorizationProgress, CancellationHandle, Lease, Model, PendingAuthorization,
    },
    model::Failure,
};
use crate::secrets::session::SecretSession;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(300);
struct Flow {
    session: Weak<SecretSession>,
    epoch: u64,
    account_id: String,
    expires: Instant,
    pending: tokio::sync::Mutex<Option<PendingAuthorization>>,
    cancellation: Mutex<Option<CancellationHandle>>,
    closed: tokio::sync::Notify,
}
impl Flow {
    fn belongs_to(&self, session: &Arc<SecretSession>) -> bool {
        self.session
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, session))
            && self.epoch == session.lifecycle_epoch()
    }
    async fn cancel(&self) {
        let handle = self
            .cancellation
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        if let Some(handle) = handle {
            handle.cancel().await;
        }
        if let Ok(mut pending) = self.pending.try_lock() {
            pending.take();
        }
        self.closed.notify_one();
    }
}
fn flows() -> &'static Mutex<HashMap<String, Arc<Flow>>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Arc<Flow>>>> = OnceLock::new();
    FLOWS.get_or_init(Default::default)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Login {
    flow_id: String,
    account_id: String,
    verification_uri: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Progress {
    Waiting,
    Saved,
}
#[derive(Serialize)]
pub(crate) struct PollResult {
    state: Progress,
}

pub(crate) async fn begin(
    session: Arc<SecretSession>,
    account_id: String,
) -> Result<Login, Failure> {
    // Reserve before the first await. A later begin supersedes this exact flow,
    // even when the earlier auth/state HTTP response arrives last.
    let id = uuid::Uuid::new_v4().to_string();
    let flow = Arc::new(Flow {
        session: Arc::downgrade(&session),
        epoch: session.lifecycle_epoch(),
        account_id: account_id.clone(),
        expires: Instant::now() + TTL,
        pending: Default::default(),
        cancellation: Default::default(),
        closed: Default::default(),
    });
    let retired = {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        let keys: Vec<_> = map
            .iter()
            .filter(|(_, old)| {
                old.expires <= Instant::now()
                    || (old.belongs_to(&session) && old.account_id == account_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        // Enforce the bound before modifying unrelated entries.
        if map.len().saturating_sub(keys.len()) >= 64 {
            return Err(Failure::Busy);
        }
        let retired: Vec<_> = keys.iter().filter_map(|key| map.remove(key)).collect();
        map.insert(id.clone(), flow.clone());
        retired
    };
    for old in retired {
        old.cancel().await;
    }
    let mut pending = match PendingAuthorization::begin(session.clone(), &account_id).await {
        Ok(pending) => Some(pending),
        Err(error) => {
            flows().lock().map_err(|_| Failure::Busy)?.remove(&id);
            return Err(error);
        }
    };
    let handle = pending
        .as_ref()
        .ok_or(Failure::Unconfirmed)?
        .cancel_handle();
    let verification_uri = pending
        .as_ref()
        .ok_or(Failure::Unconfirmed)?
        .verification_uri
        .clone();
    let installed = {
        let map = flows().lock().map_err(|_| Failure::Busy)?;
        if map
            .get(&id)
            .is_some_and(|current| Arc::ptr_eq(current, &flow))
            && flow.belongs_to(&session)
            && flow.expires > Instant::now()
        {
            *flow.cancellation.lock().map_err(|_| Failure::Busy)? = Some(handle.clone());
            *flow.pending.try_lock().map_err(|_| Failure::Busy)? = pending.take();
            true
        } else {
            false
        }
    };
    if !installed {
        handle.cancel().await;
        flows().lock().map_err(|_| Failure::Busy)?.remove(&id);
        return Err(Failure::ExpiredAuthorization);
    }
    let timer_id = id.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep_until(flow.expires.into()) => (),
            _ = flow.closed.notified() => return,
        }
        let retired = flows()
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&timer_id));
        if let Some(retired) = retired {
            retired.cancel().await;
        }
    });
    Ok(Login {
        flow_id: id,
        account_id,
        verification_uri,
    })
}

pub(crate) async fn poll(
    session: Arc<SecretSession>,
    flow_id: &str,
) -> Result<PollResult, Failure> {
    let flow = {
        let map = flows().lock().map_err(|_| Failure::Busy)?;
        let flow = map.get(flow_id).ok_or(Failure::ExpiredAuthorization)?;
        if !flow.belongs_to(&session) || flow.expires <= Instant::now() {
            return Err(Failure::ExpiredAuthorization);
        }
        flow.clone()
    };
    let result = {
        let mut slot = flow.pending.try_lock().map_err(|_| Failure::Busy)?;
        let pending = slot.as_mut().ok_or(Failure::Busy)?;
        let result = pending.poll().await;
        if !matches!(result, Ok(AuthorizationProgress::Waiting)) {
            slot.take();
        }
        result
    };
    if !matches!(result, Ok(AuthorizationProgress::Waiting)) {
        flows().lock().map_err(|_| Failure::Busy)?.remove(flow_id);
        flow.cancel().await;
    }
    result.map(|state| PollResult {
        state: match state {
            AuthorizationProgress::Waiting => Progress::Waiting,
            AuthorizationProgress::Saved => Progress::Saved,
        },
    })
}

pub(crate) async fn cancel(session: Arc<SecretSession>, flow_id: &str) -> Result<(), Failure> {
    let flow = {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        if let Some(flow) = map.get(flow_id) {
            // A stale caller may close its own retired flow. No other session or
            // newly opened flow is removed or cancelled by this identifier.
            if !flow
                .session
                .upgrade()
                .is_some_and(|owner| Arc::ptr_eq(&owner, &session))
            {
                return Err(Failure::ExpiredAuthorization);
            }
        }
        map.remove(flow_id)
    };
    if let Some(flow) = flow {
        flow.cancel().await;
    }
    Ok(())
}

pub(crate) async fn accounts(
    session: &SecretSession,
) -> Result<Vec<inference::InferenceAccount>, Failure> {
    inference::list_inference_accounts(session).await
}
pub(crate) async fn models(
    session: Arc<SecretSession>,
    account_id: &str,
) -> Result<Vec<Model>, Failure> {
    Lease::acquire(session, account_id)
        .await?
        .discover_models()
        .await
        .map_err(|_| Failure::Unconfirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Arc<SecretSession>) {
        let root = tempfile::tempdir().unwrap();
        let session = SecretSession::from_context(
            root.path().into(),
            crate::secrets::VaultContext::generate().unwrap(),
        );
        (root, session)
    }
    fn reserve(session: &Arc<SecretSession>, account_id: &str) -> (String, Arc<Flow>) {
        let id = uuid::Uuid::new_v4().to_string();
        let flow = Arc::new(Flow {
            session: Arc::downgrade(session),
            epoch: session.lifecycle_epoch(),
            account_id: account_id.into(),
            expires: Instant::now() + TTL,
            pending: Default::default(),
            cancellation: Default::default(),
            closed: Default::default(),
        });
        flows().lock().unwrap().insert(id.clone(), flow.clone());
        (id, flow)
    }
    #[tokio::test]
    async fn cancelling_exact_old_flow_never_removes_reopened_or_other_account_flow() {
        let (_root, session) = fixture();
        let (old, _) = reserve(&session, "synthetic-a");
        let (new, _) = reserve(&session, "synthetic-a");
        let (other, _) = reserve(&session, "synthetic-b");
        cancel(session.clone(), &old).await.unwrap();
        cancel(session.clone(), &old).await.unwrap();
        assert!(flows().lock().unwrap().contains_key(&new));
        assert!(flows().lock().unwrap().contains_key(&other));
        cancel(session.clone(), &new).await.unwrap();
        cancel(session, &other).await.unwrap();
    }
    #[tokio::test]
    async fn different_session_cannot_poll_or_cancel_a_pending_flow() {
        let (_root, session) = fixture();
        let (_other_root, other) = fixture();
        let (id, _) = reserve(&session, "synthetic-a");
        assert!(matches!(
            poll(other.clone(), &id).await,
            Err(Failure::ExpiredAuthorization)
        ));
        assert_eq!(cancel(other, &id).await, Err(Failure::ExpiredAuthorization));
        assert!(flows().lock().unwrap().contains_key(&id));
        cancel(session, &id).await.unwrap();
    }
    #[tokio::test]
    async fn session_epoch_retirement_blocks_poll_but_allows_own_flow_cleanup() {
        let (_root, session) = fixture();
        let (id, _) = reserve(&session, "synthetic-a");
        session.set_blocked(true);
        session.set_blocked(false);
        assert!(matches!(
            poll(session.clone(), &id).await,
            Err(Failure::ExpiredAuthorization)
        ));
        cancel(session, &id).await.unwrap();
        assert!(!flows().lock().unwrap().contains_key(&id));
    }
}
