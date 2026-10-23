//! Selected pool entries dispatch through their existing private account owner.
use super::{hyper_client::ProxyResponse, response_lease::ResponseLease, ProxyError};
use crate::{provider::Provider, secrets::session::SecretSession};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

#[derive(Clone, Copy)]
pub(crate) enum AccountKind {
    MiniMax,
    ZCode,
    WorkBuddy,
    Trae,
}

pub(crate) fn selected(provider: &Provider) -> Option<(AccountKind, String)> {
    let meta = provider.meta.as_ref()?;
    [
        (AccountKind::MiniMax, "minimax_code"),
        (AccountKind::ZCode, "zcode_start_plan"),
        (AccountKind::WorkBuddy, "workbuddy"),
        (AccountKind::Trae, "trae"),
    ]
    .into_iter()
    .find_map(|(kind, name)| {
        meta.managed_account_id_for(name)
            .filter(|id| !id.trim().is_empty())
            .map(|id| (kind, id))
    })
}
pub(crate) fn is_anthropic(provider: &Provider) -> bool {
    matches!(
        selected(provider),
        Some((AccountKind::MiniMax | AccountKind::ZCode, _))
    )
}
pub(crate) fn is_chat(provider: &Provider) -> bool {
    matches!(
        selected(provider),
        Some((AccountKind::WorkBuddy | AccountKind::Trae, _))
    )
}

pub(crate) fn validate_capabilities(
    provider: &Provider,
    body: &Value,
    automatic_failover: bool,
) -> Result<(), ProxyError> {
    if !matches!(selected(provider), Some((AccountKind::Trae, _))) {
        return Ok(());
    }
    #[cfg(any(feature = "gui", test))]
    {
        crate::trae_accounts::inference::validate_capabilities(body)
            .and_then(|()| {
                if automatic_failover {
                    crate::trae_accounts::inference::validate_automatic_capabilities(body)
                } else {
                    Ok(())
                }
            })
            .map_err(|_| ProxyError::InvalidRequest("trae.text_only_request_required".into()))
    }
    #[cfg(not(any(feature = "gui", test)))]
    {
        let _ = (body, automatic_failover);
        Err(ProxyError::ConfigError(
            "TRAE owner is unavailable in this build".into(),
        ))
    }
}

#[cfg(any(feature = "gui", test))]
enum AccountLease {
    MiniMax(crate::minimax_code::inference::Lease),
    #[cfg(any(not(test), unix))]
    ZCode(crate::zcode_accounts::inference::Lease),
    WorkBuddy(crate::workbuddy::inference::Lease),
    Trae(crate::trae_accounts::inference::Lease),
}

pub(crate) struct PreparedAccount {
    #[cfg(any(feature = "gui", test))]
    lease: AccountLease,
}

impl PreparedAccount {
    #[cfg(any(feature = "gui", test))]
    pub(crate) async fn acquire(
        session: Arc<SecretSession>,
        kind: AccountKind,
        account_id: &str,
        request_session: &str,
        _extensions: &http::Extensions,
    ) -> Result<Self, ProxyError> {
        let lease = match kind {
            AccountKind::MiniMax => AccountLease::MiniMax(
                crate::minimax_code::inference::Lease::acquire(
                    session,
                    account_id,
                    request_session,
                )
                .await
                .map_err(|_| ProxyError::AuthError("MiniMax Code account unavailable".into()))?,
            ),
            AccountKind::ZCode => {
                #[cfg(all(test, unix))]
                let lease = {
                    let fixture = _extensions
                        .get::<crate::zcode_accounts::inference::FixtureTransport>()
                        .ok_or_else(|| {
                            ProxyError::ConfigError(
                                "ZCode synthetic transport is required in tests".into(),
                            )
                        })?;
                    crate::zcode_accounts::inference::Lease::acquire_fixture(
                        session,
                        account_id,
                        request_session,
                        fixture,
                    )
                    .await
                };
                #[cfg(all(test, not(unix)))]
                return Err(ProxyError::ConfigError(
                    "ZCode synthetic owner is unsupported on this test platform".into(),
                ));
                #[cfg(not(test))]
                let lease = crate::zcode_accounts::inference::Lease::acquire(
                    session,
                    account_id,
                    request_session,
                )
                .await;
                #[cfg(any(not(test), unix))]
                {
                    AccountLease::ZCode(lease.map_err(|_| {
                        ProxyError::AuthError("ZCode Start Plan account unavailable".into())
                    })?)
                }
            }
            AccountKind::Trae => AccountLease::Trae(
                crate::trae_accounts::inference::Lease::acquire(
                    session,
                    account_id,
                    request_session,
                )
                .await
                .map_err(|_| ProxyError::Internal("trae.account_unavailable".into()))?,
            ),
            AccountKind::WorkBuddy => AccountLease::WorkBuddy(
                crate::workbuddy::inference::Lease::acquire(session, account_id)
                    .await
                    .map_err(|_| ProxyError::AuthError("WorkBuddy account unavailable".into()))?,
            ),
        };
        Ok(Self { lease })
    }

    #[cfg(not(any(feature = "gui", test)))]
    pub(crate) async fn acquire(
        _session: Arc<SecretSession>,
        _kind: AccountKind,
        _account_id: &str,
        _request_session: &str,
        _extensions: &http::Extensions,
    ) -> Result<Self, ProxyError> {
        Err(ProxyError::ConfigError(
            "Managed account owner is unavailable in this build".into(),
        ))
    }

    #[cfg(any(feature = "gui", test))]
    pub(crate) fn base_url(&self) -> &'static str {
        match &self.lease {
            AccountLease::MiniMax(lease) => lease.base_url(),
            #[cfg(any(not(test), unix))]
            AccountLease::ZCode(_) => "https://zcode.z.ai/api/v1/zcode-plan/anthropic",
            AccountLease::WorkBuddy(lease) => lease.base_url(),
            AccountLease::Trae(lease) => lease.base_url(),
        }
    }

    #[cfg(not(any(feature = "gui", test)))]
    pub(crate) fn base_url(&self) -> &'static str {
        unreachable!("disabled owner cannot be acquired")
    }

    #[cfg(any(feature = "gui", test))]
    pub(crate) async fn dispatch(
        self,
        body: Value,
        timeout: Duration,
        _extensions: &http::Extensions,
    ) -> Result<(ProxyResponse, ResponseLease), ProxyError> {
        match self.lease {
            AccountLease::MiniMax(lease) => {
                #[cfg(test)]
                let result = {
                    let fixture = _extensions
                        .get::<crate::minimax_code::inference::FixtureTransport>()
                        .ok_or_else(|| {
                            ProxyError::ConfigError(
                                "managed account synthetic transport is required in tests".into(),
                            )
                        })?;
                    lease.dispatch_fixture(body, timeout, fixture).await?
                };
                #[cfg(not(test))]
                let result = lease.dispatch(body, timeout).await?;
                let fence = result.fence;
                Ok((
                    result.response,
                    ResponseLease::new(move || {
                        let fence = fence.clone();
                        async move { fence.check().await }
                    }),
                ))
            }
            AccountLease::Trae(lease) => {
                crate::trae_accounts::protocol::validate_request(&body).map_err(|_| {
                    ProxyError::InvalidRequest("trae.text_only_request_required".into())
                })?;
                #[cfg(test)]
                let result = {
                    let fixture = _extensions
                        .get::<crate::trae_accounts::inference::FixtureTransport>()
                        .ok_or_else(|| {
                            ProxyError::ConfigError(
                                "TRAE synthetic transport is required in tests".into(),
                            )
                        })?;
                    lease.dispatch_fixture(body, timeout, fixture).await?
                };
                #[cfg(not(test))]
                let result = lease.dispatch(body, timeout).await?;
                let fence = result.fence;
                Ok((
                    result.response,
                    ResponseLease::new(move || {
                        let fence = fence.clone();
                        async move {
                            fence.check().await.map_err(|_| {
                                ProxyError::Internal("trae.response_authorization_changed".into())
                            })
                        }
                    }),
                ))
            }
            AccountLease::WorkBuddy(lease) => {
                #[cfg(test)]
                let result = {
                    let fixture = _extensions
                        .get::<crate::workbuddy::inference::FixtureTransport>()
                        .ok_or_else(|| {
                            ProxyError::ConfigError(
                                "managed account synthetic transport is required in tests".into(),
                            )
                        })?;
                    lease.dispatch_fixture(body, timeout, fixture).await?
                };
                #[cfg(not(test))]
                let result = lease.dispatch(body, timeout).await?;
                let fence = result.fence;
                Ok((
                    result.response,
                    ResponseLease::new(move || {
                        let fence = fence.clone();
                        async move { fence.check().await }
                    }),
                ))
            }
            #[cfg(any(not(test), unix))]
            AccountLease::ZCode(lease) => {
                #[cfg(all(test, unix))]
                let result = {
                    let fixture = _extensions
                        .get::<crate::zcode_accounts::inference::FixtureTransport>()
                        .ok_or_else(|| {
                            ProxyError::ConfigError(
                                "ZCode synthetic transport is required in tests".into(),
                            )
                        })?;
                    lease.dispatch_fixture(body, timeout, fixture).await?
                };
                #[cfg(not(test))]
                let result = lease.dispatch(body, timeout).await?;
                #[cfg(any(not(test), unix))]
                {
                    let fence = result.fence;
                    Ok((
                        result.response,
                        ResponseLease::new(move || {
                            let fence = fence.clone();
                            async move {
                                fence.check().await.map_err(|_| {
                                    ProxyError::Internal("zcode.account_no_longer_available".into())
                                })
                            }
                        }),
                    ))
                }
            }
        }
    }

    pub(crate) fn api_format(&self) -> &'static str {
        #[cfg(any(feature = "gui", test))]
        {
            match &self.lease {
                AccountLease::WorkBuddy(_) | AccountLease::Trae(_) => "openai_chat",
                _ => "anthropic",
            }
        }
        #[cfg(not(any(feature = "gui", test)))]
        {
            unreachable!("disabled owner cannot be acquired")
        }
    }

    /// The generic Chat converters request optional usage automatically. TRAE
    /// does not supply usage: remove only that generated option, never an
    /// explicit caller requirement (which admission rejects).
    pub(crate) fn prepare_body(&self, mut converted: Value, original: &Value) -> Value {
        #[cfg(any(feature = "gui", test))]
        if matches!(&self.lease, AccountLease::Trae(_))
            && original
                .pointer("/stream_options/include_usage")
                .and_then(Value::as_bool)
                != Some(true)
        {
            if let Some(options) = converted
                .get_mut("stream_options")
                .and_then(Value::as_object_mut)
            {
                options.remove("include_usage");
                if options.is_empty() {
                    converted
                        .as_object_mut()
                        .expect("converted request object")
                        .remove("stream_options");
                }
            }
        }
        #[cfg(not(any(feature = "gui", test)))]
        let _ = original;
        converted
    }

    pub(crate) fn accepts_endpoint(&self, endpoint: &str) -> bool {
        #[cfg(any(feature = "gui", test))]
        {
            match &self.lease {
                AccountLease::WorkBuddy(_) | AccountLease::Trae(_) => {
                    matches!(endpoint, "/chat/completions" | "/v1/chat/completions")
                }
                _ => matches!(endpoint, "/messages" | "/v1/messages"),
            }
        }
        #[cfg(not(any(feature = "gui", test)))]
        {
            let _ = endpoint;
            false
        }
    }

    #[cfg(not(any(feature = "gui", test)))]
    pub(crate) async fn dispatch(
        self,
        _body: Value,
        _timeout: Duration,
        _extensions: &http::Extensions,
    ) -> Result<(ProxyResponse, ResponseLease), ProxyError> {
        Err(ProxyError::ConfigError(
            "Managed account owner is unavailable in this build".into(),
        ))
    }
}

#[cfg(all(test, not(unix)))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn zcode_non_unix_fixture_acquisition_remains_closed() {
        let result = PreparedAccount::acquire(
            SecretSession::ephemeral().unwrap(),
            AccountKind::ZCode,
            "synthetic-account",
            "synthetic-session",
            &http::Extensions::new(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProxyError::ConfigError(message))
                if message == "ZCode synthetic owner is unsupported on this test platform"
        ));
    }
}
