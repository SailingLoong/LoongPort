//! Independent CN device grants. No official credential file is used.
//! Campaign check-in is a separate, unsupported CN capability.
use super::qoder_daily::http::{CredentialSource, PatDeclaration};
use super::qoder_http_auth::{
    self as owner, DeviceIdentityAction, EstablishLease, IdentityAction, IdentityActionRequest,
    IdentityConfirmation, IdentityStatus, PreparedIdentityAction, SourceKind,
};
use crate::{database::Database, error::AppError, provider::Provider};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, Weak},
};
use zeroize::Zeroizing;

const LOGIN_TTL_MS: i64 = 300_000;
const POLL_INTERVAL_MS: u64 = 1000;
const MAX_SESSIONS: usize = 64;
// Public production OAuth client identifier, not a secret. Qoder CN CLI 1.1.65.
const PRODUCTION_CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartRequest {
    pub(crate) provider_id: Option<String>,
    pub(crate) credential_generation: Option<String>,
    pub(crate) rebind: bool,
}
#[derive(Debug, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum StartResult {
    Started {
        session_id: String,
        provider_id: String,
        credential_generation: String,
        auth_url: String,
        expires_at_ms: i64,
        poll_interval_ms: u64,
    },
    Failed {
        code: &'static str,
    },
}
#[derive(Debug, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum PollResult {
    Pending,
    Completed { provider_id: String },
    Failed { code: &'static str },
}
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum CancelResult {
    Cancelled,
}
trait DevicePoll: Send + Sync {
    fn poll<'a>(
        &'a self,
        nonce: &'a str,
        verifier: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<owner::Established>, ()>> + Send + 'a>,
    >;
}
impl DevicePoll for owner::transport::FixedRegionTransport {
    fn poll<'a>(
        &'a self,
        nonce: &'a str,
        verifier: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<owner::Established>, ()>> + Send + 'a>,
    > {
        Box::pin(self.poll_device_login(nonce, verifier))
    }
}
struct Session {
    db: Weak<Database>,
    lease: EstablishLease,
    nonce: String,
    verifier: Zeroizing<String>,
    expires_at_ms: i64,
    progress: Mutex<Progress>,
}
enum Progress {
    Waiting { last_poll_ms: Option<i64> },
    InFlight,
    Completed,
    Failed(&'static str),
    Cancelled,
}
#[derive(Default)]
pub(crate) struct DeviceLoginSessions {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}
pub(crate) fn sessions() -> &'static DeviceLoginSessions {
    static SESSIONS: OnceLock<DeviceLoginSessions> = OnceLock::new();
    SESSIONS.get_or_init(DeviceLoginSessions::default)
}
fn new_provider() -> Provider {
    let mut provider = Provider::with_id(
        format!("qoder-cn-device-{}", uuid::Uuid::new_v4()),
        "Qoder CN 独立授权".into(),
        serde_json::json!({
            "auth": {},
            "config": "model_provider = \"QoderCN\"\n[model_providers.QoderCN]\nname = \"Qoder CN\"\nbase_url = \"https://openapi.qoder.com.cn\"\nwire_api = \"responses\"\n"
        }),
        Some("https://qoder.cn".into()),
    );
    provider.category = Some("third_party".into());
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_cn_device".into()),
        ..Default::default()
    });
    provider
}
impl DeviceLoginSessions {
    fn lookup(&self, db: &Arc<Database>, session_id: &str) -> Result<Arc<Session>, ()> {
        let session = self
            .sessions
            .lock()
            .map_err(|_| ())?
            .get(session_id)
            .cloned()
            .ok_or(())?;
        if !session
            .db
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, db))
        {
            return Err(());
        }
        Ok(session)
    }
    pub(crate) async fn poll(&self, db: &Arc<Database>, session_id: &str) -> PollResult {
        let transport = match owner::transport::FixedRegionTransport::for_source(
            SourceKind::ChinaQoderDevice,
        ) {
            Ok(transport) => transport,
            Err(_) => {
                return PollResult::Failed {
                    code: "unavailable",
                }
            }
        };
        self.poll_using(db, session_id, &transport, || {
            chrono::Utc::now().timestamp_millis()
        })
        .await
    }
    async fn poll_using(
        &self,
        db: &Arc<Database>,
        session_id: &str,
        transport: &impl DevicePoll,
        now: impl Fn() -> i64,
    ) -> PollResult {
        let session = match self.lookup(db, session_id) {
            Ok(session) => session,
            Err(_) => {
                return PollResult::Failed {
                    code: "invalid_session",
                }
            }
        };
        {
            let Ok(mut progress) = session.progress.lock() else {
                return PollResult::Failed {
                    code: "unavailable",
                };
            };
            let now_ms = now();
            match *progress {
                Progress::Completed => {
                    return PollResult::Completed {
                        provider_id: session.lease.stamp.source.provider_id.clone(),
                    }
                }
                Progress::Failed(code) => return PollResult::Failed { code },
                Progress::Cancelled => return PollResult::Failed { code: "cancelled" },
                _ if now_ms < 0 || now_ms >= session.expires_at_ms => {
                    return Self::fail(db, &session, &mut progress, "expired");
                }
                Progress::InFlight => return PollResult::Pending,
                Progress::Waiting {
                    last_poll_ms: Some(last),
                } if now_ms.saturating_sub(last) < POLL_INTERVAL_MS as i64 => {
                    return PollResult::Pending
                }
                Progress::Waiting { .. } => {
                    if db.check_qoder_http_establish(&session.lease).is_err() {
                        return Self::fail(db, &session, &mut progress, "source_changed");
                    }
                    *progress = Progress::InFlight;
                }
            }
        }
        // No vault/SQLite/session mutex is held during the one fixed-origin GET.
        let response = transport.poll(&session.nonce, &session.verifier).await;
        let Ok(mut progress) = session.progress.lock() else {
            return PollResult::Failed {
                code: "unavailable",
            };
        };
        if let Progress::Cancelled = *progress {
            return PollResult::Failed { code: "cancelled" };
        }
        if let Progress::Failed(code) = *progress {
            return PollResult::Failed { code };
        }
        let now_ms = now();
        if now_ms < 0 || now_ms >= session.expires_at_ms {
            return Self::fail(db, &session, &mut progress, "expired");
        }
        match response {
            Ok(None) => {
                if db.check_qoder_http_establish(&session.lease).is_err() {
                    return Self::fail(db, &session, &mut progress, "source_changed");
                }
                *progress = Progress::Waiting {
                    last_poll_ms: Some(now_ms),
                };
                PollResult::Pending
            }
            Err(()) => Self::fail(db, &session, &mut progress, "login_failed"),
            Ok(Some(established)) => {
                match db.qoder_http_publish_establish(&session.lease, established, now_ms) {
                    Ok(_) => {
                        *progress = Progress::Completed;
                        PollResult::Completed {
                            provider_id: session.lease.stamp.source.provider_id.clone(),
                        }
                    }
                    Err(error) => {
                        let code = if error.to_string().contains("identity_rebind_required") {
                            "identity_rebind_required"
                        } else {
                            "source_changed"
                        };
                        Self::fail(db, &session, &mut progress, code)
                    }
                }
            }
        }
    }
    fn fail(
        db: &Database,
        session: &Session,
        progress: &mut Progress,
        code: &'static str,
    ) -> PollResult {
        *progress = Progress::Failed(code);
        let _ = db.qoder_http_mark_unknown(&session.lease.stamp, &session.lease.attempt_id);
        PollResult::Failed { code }
    }
    pub(crate) fn cancel(&self, db: &Arc<Database>, session_id: &str) -> CancelResult {
        if let Ok(session) = self.lookup(db, session_id) {
            if let Ok(mut progress) = session.progress.lock() {
                // Cancellation wins over an in-flight response. An already committed
                // grant is terminal; cancel is not an unverified server revoke API.
                if matches!(*progress, Progress::Waiting { .. } | Progress::InFlight) {
                    *progress = Progress::Cancelled;
                    let _ =
                        db.qoder_http_mark_unknown(&session.lease.stamp, &session.lease.attempt_id);
                }
            }
        }
        CancelResult::Cancelled
    }
    fn prune(&self, now_ms: i64) -> Result<(), ()> {
        let mut expired = Vec::new();
        self.sessions.lock().map_err(|_| ())?.retain(|_, session| {
            if session.expires_at_ms <= now_ms || session.db.upgrade().is_none() {
                expired.push(session.clone());
                false
            } else {
                true
            }
        });
        for session in expired {
            if let (Some(db), Ok(mut progress)) = (session.db.upgrade(), session.progress.lock()) {
                if matches!(*progress, Progress::Waiting { .. } | Progress::InFlight) {
                    *progress = Progress::Failed("expired");
                    let _ =
                        db.qoder_http_mark_unknown(&session.lease.stamp, &session.lease.attempt_id);
                }
            }
        }
        Ok(())
    }
    pub(crate) fn start(
        &self,
        db: Arc<Database>,
        request: StartRequest,
        add: impl FnOnce(Provider) -> Result<(), AppError>,
        confirm: impl FnOnce(IdentityConfirmation) -> bool,
        now_ms: i64,
    ) -> StartResult {
        let failed = |code| StartResult::Failed { code };
        if now_ms < 0
            || now_ms.checked_add(LOGIN_TTL_MS).is_none()
            || !matches!(
                (&request.provider_id, &request.credential_generation),
                (None, None) | (Some(_), Some(_))
            )
            || (request.provider_id.is_none() && request.rebind)
        {
            return failed("invalid_request");
        }
        if self.prune(now_ms).is_err() {
            return failed("unavailable");
        }
        match self.sessions.lock() {
            Ok(sessions) if sessions.len() < MAX_SESSIONS => {}
            _ => return failed("too_many_sessions"),
        }
        // The verifier is ephemeral, never returned through IPC or persisted.
        let mut entropy = Zeroizing::new([0_u8; 32]);
        if getrandom::fill(entropy.as_mut()).is_err() {
            return failed("unavailable");
        }
        let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(entropy.as_ref()));
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let (provider_id, expected_generation) = match request.provider_id {
            Some(id) => (id, request.credential_generation),
            None => {
                let provider = new_provider();
                let id = provider.id.clone();
                if add(provider).is_err() {
                    return failed("save_failed");
                }
                (id, None)
            }
        };
        let provider = match db.get_provider_by_id(&provider_id, "codex") {
            Ok(Some(provider)) => provider,
            _ => return failed("unavailable"),
        };
        let source = match owner::SourceBinding::from_provider(&provider) {
            Ok((source, _)) if source.source == SourceKind::ChinaQoderDevice => source,
            _ => return failed("source_changed"),
        };
        if expected_generation
            .as_ref()
            .is_some_and(|generation| *generation != source.credential_generation)
        {
            return failed("source_changed");
        }
        let context = match db.qoder_http_identity_context(&provider_id, now_ms) {
            Ok(context) => context,
            Err(_) => return failed("unavailable"),
        };
        let wanted = if request.rebind {
            DeviceIdentityAction::Rebind
        } else {
            DeviceIdentityAction::Login
        };
        if !context.allowed_device_actions.contains(&wanted) {
            return failed("action_unavailable");
        }
        let action = if request.rebind {
            IdentityAction::Rebind
        } else if matches!(
            context.status,
            IdentityStatus::Pending | IdentityStatus::Unknown
        ) {
            IdentityAction::Recover
        } else {
            IdentityAction::Establish
        };
        let prepared = db.qoder_http_prepare_identity_action(
            &IdentityActionRequest {
                declaration: PatDeclaration {
                    provider_id: provider_id.clone(),
                    credential_generation: source.credential_generation.clone(),
                    source: CredentialSource::ChinaQoderDevice,
                },
                action,
            },
            now_ms,
        );
        let (receipt, confirmation) = match prepared {
            Ok(PreparedIdentityAction::Establish {
                receipt,
                confirmation,
            }) => (receipt, confirmation),
            _ => return failed("source_changed"),
        };
        if confirmation.is_some_and(|summary| !confirm(summary)) {
            return failed("cancelled");
        }
        let lease = match db.qoder_http_reserve_identity_action(receipt) {
            Ok(lease) => lease,
            Err(_) => return failed("source_changed"),
        };
        let nonce = uuid::Uuid::new_v4().to_string();
        let mut auth_url =
            reqwest::Url::parse("https://qoder.cn/device/selectAccounts").expect("fixed CN URL");
        auth_url
            .query_pairs_mut()
            .append_pair("challenge", &challenge)
            .append_pair("challenge_method", "S256")
            .append_pair("nonce", &nonce)
            .append_pair("machine_id", &lease.machine_id)
            .append_pair("client_id", PRODUCTION_CLIENT_ID);
        let session_id = uuid::Uuid::new_v4().to_string();
        let expires_at_ms = now_ms + LOGIN_TTL_MS;
        let session = Arc::new(Session {
            db: Arc::downgrade(&db),
            lease,
            nonce,
            verifier,
            expires_at_ms,
            progress: Mutex::new(Progress::Waiting { last_poll_ms: None }),
        });
        match self.sessions.lock() {
            Ok(mut sessions) if sessions.len() < MAX_SESSIONS => {
                sessions.insert(session_id.clone(), session);
            }
            _ => {
                let _ = db.qoder_http_mark_unknown(&session.lease.stamp, &session.lease.attempt_id);
                return failed("too_many_sessions");
            }
        }
        StartResult::Started {
            session_id,
            provider_id,
            credential_generation: source.credential_generation,
            auth_url: auth_url.into(),
            expires_at_ms,
            poll_interval_ms: POLL_INTERVAL_MS,
        }
    }
}
#[cfg(test)]
#[path = "qoder_cn_device_login_tests.rs"]
mod tests;
