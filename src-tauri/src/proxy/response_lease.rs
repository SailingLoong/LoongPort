//! Ephemeral account-validity check carried to the final downstream body.
//! This carries no credentials or independent account version/state.
use super::ProxyError;
use std::{future::Future, pin::Pin, sync::Arc};

type Check = Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>>;

#[derive(Clone)]
pub(crate) struct ResponseLease(Arc<dyn Fn() -> Check + Send + Sync>);

impl ResponseLease {
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
