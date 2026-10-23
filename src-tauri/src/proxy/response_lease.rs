//! Ephemeral account-validity check carried to the final downstream body.
//! This carries no credentials or independent account version/state.
use super::ProxyError;
use std::{future::Future, pin::Pin, sync::Arc};

type Check = Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>>;

#[derive(Clone)]
pub(crate) struct ResponseLease(Arc<dyn Fn() -> Check + Send + Sync>);

impl ResponseLease {
    #[cfg(any(feature = "gui", feature = "test-hooks", test))]
    pub(crate) fn combine(first: Option<Self>, second: Option<Self>) -> Option<Self> {
        match (first, second) {
            (None, lease) | (lease, None) => lease,
            (Some(first), Some(second)) => Some(Self::new(move || {
                let first = first.clone();
                let second = second.clone();
                async move {
                    first.check().await?;
                    second.check().await
                }
            })),
        }
    }

    #[cfg(any(feature = "gui", feature = "test-hooks", test))]
    pub(crate) fn new<F, Fut>(check: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ProxyError>> + Send + 'static,
    {
        Self(Arc::new(move || Box::pin(check())))
    }

    pub(crate) async fn check(&self) -> Result<(), ProxyError> {
        match (self.0)().await {
            // This error originates only in the selected private owner's fence,
            // never from an upstream-provided error string.
            Err(error @ ProxyError::UpstreamError { status: 403, .. }) => Err(error),
            Err(_) => Err(ProxyError::Internal(
                "managed_response.authorization_or_output_blocked".into(),
            )),
            Ok(()) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn combined_lease_checks_both_owners_and_denies_revocation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let first = ResponseLease::new({
            let calls = calls.clone();
            move || {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        });
        let second = ResponseLease::new({
            let calls = calls.clone();
            move || {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(ProxyError::AuthError("revoked".into()))
                }
            }
        });
        let combined = ResponseLease::combine(Some(first), Some(second)).unwrap();
        assert!(combined.check().await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
