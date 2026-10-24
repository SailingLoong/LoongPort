//! One local capacity owner across applications and keys for a known account.
//! The configured limit is local policy, not a claim about the vendor's quota.
use crate::{
    database::Database,
    provider::{AuthBindingSource, Provider},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

const POLICY_KEY: &str = "proxy_account_concurrency_limit";
const DEFAULT_LOCAL_LIMIT: usize = 1;

#[derive(Hash, PartialEq, Eq)]
enum AccountIdentity {
    Managed { authority: String, account: String },
    Site(String),
}

fn account_identity(provider: &Provider) -> Option<AccountIdentity> {
    if let Some(key) = provider.failover_account_key() {
        return Some(AccountIdentity::Site(key));
    }
    let binding = provider.meta.as_ref()?.auth_binding.as_ref()?;
    if binding.source != AuthBindingSource::ManagedAccount {
        return None;
    }
    let authority = binding
        .auth_provider
        .as_deref()
        .filter(|s| !s.trim().is_empty())?;
    let account = binding
        .account_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())?;
    Some(AccountIdentity::Managed {
        authority: authority.into(),
        account: account.into(),
    })
}

/// Busy is local admission state, never an upstream attempt or owner error.
pub(crate) enum CapacityAdmission {
    Acquired(Option<OwnedSemaphorePermit>),
    Busy,
}

pub(crate) struct AccountCapacity {
    // Read once by the proxy owner; policy changes take effect on proxy restart.
    limit: Result<usize, &'static str>,
    lanes: Mutex<HashMap<AccountIdentity, Weak<Semaphore>>>,
}

impl Default for AccountCapacity {
    fn default() -> Self {
        Self {
            limit: Ok(DEFAULT_LOCAL_LIMIT),
            lanes: Mutex::new(HashMap::new()),
        }
    }
}

impl AccountCapacity {
    pub(crate) fn from_database(db: &Database) -> Self {
        let limit = db
            .get_setting(POLICY_KEY)
            .map_err(|_| "account_capacity.policy_unavailable")
            .and_then(|raw| match raw {
                None => Ok(DEFAULT_LOCAL_LIMIT),
                Some(raw) => raw
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0 && *n <= Semaphore::MAX_PERMITS)
                    .ok_or("account_capacity.invalid_limit"),
            });
        Self {
            limit,
            lanes: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn applies_to(provider: &Provider) -> bool {
        account_identity(provider).is_some()
    }

    fn lane_for(&self, provider: &Provider) -> Result<Option<Arc<Semaphore>>, super::ProxyError> {
        let Some(account) = account_identity(provider) else {
            return Ok(None);
        };
        let limit = self
            .limit
            .map_err(|code| super::ProxyError::ConfigError(code.into()))?;
        let mut lanes = self.lanes.lock().map_err(|_| {
            super::ProxyError::Internal("account_capacity.owner_unavailable".into())
        })?;
        lanes.retain(|_, lane| lane.strong_count() > 0);
        let lane = match lanes.get(&account).and_then(Weak::upgrade) {
            Some(lane) => lane,
            None => {
                let lane = Arc::new(Semaphore::new(limit));
                lanes.insert(account, Arc::downgrade(&lane));
                lane
            }
        };
        Ok(Some(lane))
    }

    pub(crate) fn try_acquire(
        &self,
        provider: &Provider,
    ) -> Result<CapacityAdmission, super::ProxyError> {
        let Some(lane) = self.lane_for(provider)? else {
            return Ok(CapacityAdmission::Acquired(None));
        };
        match lane.try_acquire_owned() {
            Ok(permit) => Ok(CapacityAdmission::Acquired(Some(permit))),
            Err(TryAcquireError::NoPermits) => Ok(CapacityAdmission::Busy),
            Err(TryAcquireError::Closed) => Err(super::ProxyError::Internal(
                "account_capacity.owner_stopped".into(),
            )),
        }
    }

    pub(crate) async fn acquire(
        &self,
        provider: &Provider,
    ) -> Result<Option<OwnedSemaphorePermit>, super::ProxyError> {
        let Some(lane) = self.lane_for(provider)? else {
            // No reliable account identity means no invented sharing relation.
            return Ok(None);
        };
        lane.acquire_owned()
            .await
            .map(Some)
            .map_err(|_| super::ProxyError::Internal("account_capacity.owner_stopped".into()))
    }
}

/// Shared only by a single request's attempts and final response-body owner.
/// Failed attempts clear their permit before trying another authorized provider.
#[derive(Clone, Default)]
pub(crate) struct AccountCapacityLease(Arc<Mutex<Option<OwnedSemaphorePermit>>>);

impl AccountCapacityLease {
    pub(crate) fn hold(
        &self,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Result<(), super::ProxyError> {
        *self.0.lock().map_err(|_| {
            super::ProxyError::Internal("account_capacity.lease_unavailable".into())
        })? = permit;
        Ok(())
    }

    pub(crate) fn release(&self) -> Result<(), super::ProxyError> {
        self.hold(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AuthBinding, AuthBindingSource};
    use serde_json::json;
    use std::sync::Arc;

    fn account_provider(id: &str, account: &str) -> Provider {
        let mut provider = Provider::with_id(id.into(), "Synthetic".into(), json!({}), None);
        provider
            .meta
            .get_or_insert_with(Default::default)
            .auth_binding = Some(AuthBinding {
            source: AuthBindingSource::ManagedAccount,
            auth_provider: Some("synthetic".into()),
            account_id: Some(account.into()),
        });
        provider
    }

    #[tokio::test]
    async fn different_client_rows_for_one_account_wait_for_the_response_owner() {
        let capacity = Arc::new(AccountCapacity::default());
        let first = capacity
            .acquire(&account_provider("first", "shared"))
            .await
            .unwrap();
        let second_provider = account_provider("second", "shared");
        let mut second = Box::pin(capacity.acquire(&second_provider));
        assert!(futures::poll!(&mut second).is_pending());
        drop(first);
        assert!(second.await.unwrap().is_some());
    }

    #[tokio::test]
    async fn queued_caller_retains_the_protocol_model_and_observes_revocation() {
        let db = Arc::new(Database::memory().unwrap());
        let reference = db
            .issue_client_credential("synthetic", "gemini", &["model-a".into()])
            .unwrap();
        let key = db
            .resolve_client_credential(&reference.id, "synthetic", "gemini")
            .unwrap();
        let caller = db
            .authenticate_client(&key, "gemini", Some("model-a"))
            .unwrap();
        let router = crate::proxy::provider_router::ProviderRouter::new(db.clone());
        let provider = account_provider("synthetic", "shared");
        let first = router.acquire_account_capacity(&provider).await.unwrap();
        let mut queued = Box::pin(router.acquire_account_capacity(&provider));
        assert!(futures::poll!(&mut queued).is_pending());
        assert!(router
            .authorize_client_request(&caller, "gemini", caller.requested_model.as_deref())
            .is_ok());
        db.revoke_client_credential(&reference.id).unwrap();
        drop(first);
        let _permit = queued.await.unwrap();
        assert!(matches!(
            router.authorize_client_request(&caller, "gemini", caller.requested_model.as_deref()),
            Err(super::super::ProxyError::ClientAccessDenied)
        ));
    }

    #[tokio::test]
    async fn independent_accounts_can_run_together() {
        let capacity = AccountCapacity::default();
        let first = capacity
            .acquire(&account_provider("first", "one"))
            .await
            .unwrap();
        let second = capacity
            .acquire(&account_provider("second", "two"))
            .await
            .unwrap();
        assert!(first.is_some() && second.is_some());
    }

    #[tokio::test]
    async fn cancelled_waiter_never_consumes_the_next_slot() {
        let capacity = AccountCapacity::default();
        let provider = account_provider("first", "shared");
        let first = capacity.acquire(&provider).await.unwrap();
        let mut waiting = Box::pin(capacity.acquire(&provider));
        assert!(futures::poll!(&mut waiting).is_pending());
        drop(waiting);
        drop(first);
        assert!(capacity.acquire(&provider).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn unrelated_manual_keys_do_not_invent_an_account_relationship() {
        let capacity = AccountCapacity::default();
        let provider = Provider::with_id("manual".into(), "Synthetic".into(), json!({}), None);
        assert!(capacity.acquire(&provider).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn declared_local_policy_allows_parallel_work_without_inventing_vendor_quota() {
        let db = Database::memory().unwrap();
        db.set_setting(POLICY_KEY, "2").unwrap();
        let capacity = AccountCapacity::from_database(&db);
        let provider = account_provider("first", "shared");
        let first = capacity.acquire(&provider).await.unwrap();
        let second = capacity.acquire(&provider).await.unwrap();
        let mut third = Box::pin(capacity.acquire(&provider));
        assert!(futures::poll!(&mut third).is_pending());
        drop(first);
        assert!(third.await.unwrap().is_some());
        drop(second);
    }

    #[tokio::test]
    async fn malformed_capacity_policy_rejects_account_admission() {
        let db = Database::memory().unwrap();
        db.set_setting(POLICY_KEY, "0").unwrap();
        assert!(AccountCapacity::from_database(&db)
            .acquire(&account_provider("first", "shared"))
            .await
            .is_err());
    }
    #[tokio::test]
    async fn atomic_try_uses_the_existing_account_owner_and_distinguishes_busy() {
        let capacity = AccountCapacity::default();
        let a = account_provider("a", "shared");
        let sibling = account_provider("other-card", "shared");
        let b = account_provider("b", "independent");
        let held = capacity.acquire(&a).await.unwrap();
        assert!(matches!(
            capacity.try_acquire(&sibling).unwrap(),
            CapacityAdmission::Busy
        ));
        let available = capacity.try_acquire(&b).unwrap();
        assert!(matches!(available, CapacityAdmission::Acquired(Some(_))));
        assert!(matches!(
            capacity.try_acquire(&a).unwrap(),
            CapacityAdmission::Busy
        ));
        drop(held);
        assert!(matches!(
            capacity.try_acquire(&sibling).unwrap(),
            CapacityAdmission::Acquired(Some(_))
        ));
    }

    #[test]
    fn atomic_try_preserves_unscoped_keys_and_fails_closed_for_invalid_policy() {
        let manual = Provider::with_id("manual".into(), "Synthetic".into(), json!({}), None);
        assert!(matches!(
            AccountCapacity::default().try_acquire(&manual).unwrap(),
            CapacityAdmission::Acquired(None)
        ));
        let db = Database::memory().unwrap();
        db.set_setting(POLICY_KEY, "0").unwrap();
        assert!(AccountCapacity::from_database(&db)
            .try_acquire(&account_provider("a", "shared"))
            .is_err());
    }

    #[tokio::test]
    async fn atomic_try_honors_existing_waiters_and_does_not_steal_their_slot() {
        let capacity = AccountCapacity::default();
        let a = account_provider("a", "shared");
        let held = capacity.acquire(&a).await.unwrap();
        let mut waiting = Box::pin(capacity.acquire(&a));
        assert!(futures::poll!(&mut waiting).is_pending());
        drop(held);
        assert!(matches!(
            capacity.try_acquire(&a).unwrap(),
            CapacityAdmission::Busy
        ));
        drop(waiting.await.unwrap());
        assert!(matches!(
            capacity.try_acquire(&a).unwrap(),
            CapacityAdmission::Acquired(Some(_))
        ));
    }
}
