//! Official device authorization only; never reads or replaces native sessions.
use super::{
    checkin,
    model::*,
    protocol::{Endpoint, Transport},
    store,
};
use crate::secrets::{session::SecretSession, VaultMetadata};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Login {
    pub flow_id: String,
    pub verification_uri: String,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AuthorizationState {
    Waiting,
    Saved,
}
#[derive(Serialize)]
pub(crate) struct AuthorizationResult {
    pub state: AuthorizationState,
}
#[derive(Clone)]
struct Flow {
    state: zeroize::Zeroizing<String>,
    root: PathBuf,
    binding: VaultMetadata,
    expires: i64,
    epoch: u64,
    deadline: std::time::Instant,
    poll: std::sync::Arc<tokio::sync::Mutex<()>>,
}
fn flows() -> &'static Mutex<HashMap<String, Flow>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Flow>>> = OnceLock::new();
    FLOWS.get_or_init(Default::default)
}
fn text<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k)?.as_str())
        .filter(|s| !s.trim().is_empty())
}
pub(crate) fn parse_account(token: &Value, profile: &Value) -> Result<Account, Failure> {
    if !checkin::success(token) || !checkin::success(profile) {
        return Err(Failure::NeedsVerification);
    }
    let t = token.get("data").ok_or(Failure::Unconfirmed)?;
    let p = profile.get("data").ok_or(Failure::Unconfirmed)?;
    let token = text(t, &["accessToken", "access_token"]).ok_or(Failure::NeedsVerification)?;
    if token.len() > 16384
        || reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_err()
    {
        return Err(Failure::NeedsVerification);
    }
    let domain = Domain::parse(text(t, &["domain"]).ok_or(Failure::UnsupportedContext)?)?;
    if let Some(profile_domain) = text(p, &["domain"]) {
        if Domain::parse(profile_domain)? != domain {
            return Err(Failure::UnsupportedContext);
        }
    }
    if [t, p].iter().any(|v| {
        ["enterpriseId", "enterprise_id"].iter().any(|key| {
            v.get(*key)
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        }) || v
            .get("type")
            .is_some_and(|kind| !matches!(kind.as_str(), Some("personal" | "individual")))
    }) {
        return Err(Failure::UnsupportedContext);
    }
    let uid = text(p, &["uid"]).ok_or(Failure::UnsupportedContext)?;
    if uid.len() > 256 || uid.chars().any(char::is_control) {
        return Err(Failure::UnsupportedContext);
    }
    let label = text(p, &["nickname", "email"])
        .unwrap_or("WorkBuddy")
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect();
    Ok(Account {
        uid: uid.into(),
        domain,
        label,
        token: token.into(),
    })
}
pub(crate) fn verification_url(raw: &str, state: &str) -> Result<String, Failure> {
    let url = reqwest::Url::parse(raw).map_err(|_| Failure::UnsupportedContext)?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.codebuddy.cn")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || url.fragment().is_some()
    {
        return Err(Failure::UnsupportedContext);
    }
    let mut states = 0;
    for (key, value) in url.query_pairs() {
        if key == "state" {
            states += 1;
            if value != state {
                return Err(Failure::UnsupportedContext);
            }
        }
        if key.to_ascii_lowercase().contains("token")
            || key.to_ascii_lowercase().contains("password")
        {
            return Err(Failure::UnsupportedContext);
        }
    }
    if states != 1 || state.is_empty() || state.len() > 4096 {
        return Err(Failure::UnsupportedContext);
    }
    Ok(url.to_string())
}
// Reuse inference's first-poll admission: create and start the request while
// both lifecycle ownership and exact-flow cancellation are fenced, then release
// every shared guard before waiting on the provider.
async fn send_flow<T: Transport>(
    session: &SecretSession,
    transport: &T,
    id: &str,
    endpoint: Endpoint,
    account: Option<&Account>,
    state: Option<&str>,
    now: i64,
) -> Result<Value, Failure> {
    let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
    let mut pending = None;
    let mut ready = None;
    {
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let map = flows().lock().map_err(|_| Failure::Busy)?;
        let flow = map.get(id).ok_or(Failure::ExpiredAuthorization)?;
        if flow.root != session.root()
            || flow.binding != store::binding(session)?
            || flow.epoch != session.lifecycle_epoch()
            || flow.expires <= now
            || flow.deadline <= std::time::Instant::now()
        {
            return Err(Failure::ExpiredAuthorization);
        }
        let mut future = transport.request(endpoint, account, state, now);
        match std::future::Future::poll(
            future.as_mut(),
            &mut std::task::Context::from_waker(&waker),
        ) {
            std::task::Poll::Ready(result) => ready = Some(result),
            std::task::Poll::Pending => pending = Some(future),
        }
    }
    if let Some(result) = ready {
        result
    } else {
        pending.ok_or(Failure::Unconfirmed)?.await
    }
}

pub(crate) async fn begin<T: Transport>(
    session: &SecretSession,
    transport: &T,
    now: i64,
) -> Result<Login, Failure> {
    let owner = crate::services::sync_protocol::sync_mutex().lock().await;
    let binding = store::binding(session)?;
    let epoch = session.lifecycle_epoch();
    let flow_id = uuid::Uuid::new_v4().to_string();
    // Reserve under short admission, before HTTP. A second begin can retire
    // an older finish even while that finish is awaiting its profile.
    {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        map.retain(|_, flow| {
            flow.expires > now
                && flow.deadline > std::time::Instant::now()
                && !(flow.root == session.root() && flow.binding == binding)
        });
        if map.len() >= 64 {
            return Err(Failure::Busy);
        }
        map.insert(
            flow_id.clone(),
            Flow {
                state: zeroize::Zeroizing::new(String::new()),
                root: session.root().into(),
                binding: binding.clone(),
                expires: now + 600000,
                epoch,
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
                poll: Default::default(),
            },
        );
    }
    drop(owner);
    let result = async {
        let result = send_flow(
            session,
            transport,
            &flow_id,
            Endpoint::AuthState,
            None,
            None,
            now,
        )
        .await?;
        if !checkin::success(&result) {
            return Err(Failure::NeedsVerification);
        }
        let data = result.get("data").ok_or(Failure::Unconfirmed)?;
        let state = text(data, &["state"])
            .filter(|s| s.len() <= 4096)
            .ok_or(Failure::Unconfirmed)?;
        let raw = text(data, &["authUrl", "auth_url", "url"]).ok_or(Failure::Unconfirmed)?;
        let verification_uri = verification_url(raw, state)?;
        if session.lifecycle_epoch() != epoch || store::binding(session)? != binding {
            return Err(Failure::ExpiredAuthorization);
        }
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        let flow = map.get_mut(&flow_id).ok_or(Failure::ExpiredAuthorization)?;
        if flow.deadline <= std::time::Instant::now() {
            return Err(Failure::ExpiredAuthorization);
        }
        flow.state = zeroize::Zeroizing::new(state.into());
        Ok(Login {
            flow_id: flow_id.clone(),
            verification_uri,
        })
    }
    .await;
    if result.is_err() {
        flows().lock().map_err(|_| Failure::Busy)?.remove(&flow_id);
    }
    result
}

pub(crate) async fn finish<T: Transport>(
    session: &SecretSession,
    transport: &T,
    id: &str,
    now: i64,
) -> Result<AuthorizationResult, Failure> {
    let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
    let binding = store::binding(session)?;
    let flow = {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        let f = map.get(id).ok_or(Failure::ExpiredAuthorization)?;
        if f.root != session.root() || f.binding != binding {
            return Err(Failure::UnsupportedContext);
        }
        if f.expires <= now
            || f.deadline <= std::time::Instant::now()
            || f.epoch != session.lifecycle_epoch()
        {
            map.remove(id);
            return Err(Failure::ExpiredAuthorization);
        }
        f.clone()
    };
    // Keep each exact flow single-flight without owning the vault-wide lock
    // while awaiting the provider. Cancellation and other accounts stay live.
    let _poll = flow
        .poll
        .clone()
        .try_lock_owned()
        .map_err(|_| Failure::Busy)?;
    drop(_owner);
    let token = send_flow(
        session,
        transport,
        id,
        Endpoint::AuthToken,
        None,
        Some(&flow.state),
        now,
    )
    .await?;
    {
        let pending = flows().lock().map_err(|_| Failure::Busy)?;
        let current = pending.get(id).ok_or(Failure::ExpiredAuthorization)?;
        if current.epoch != session.lifecycle_epoch()
            || current.deadline <= std::time::Instant::now()
        {
            return Err(Failure::ExpiredAuthorization);
        }
    }
    // Official browser authorization is not an error while the token is still
    // pending. Match Magpie's token-stage code, not arbitrary provider failures.
    if checkin::code(&token) == Some(11217) {
        return Ok(AuthorizationResult {
            state: AuthorizationState::Waiting,
        });
    }
    if !checkin::success(&token) {
        return Err(Failure::NeedsVerification);
    }
    if text(
        token.get("data").unwrap_or(&Value::Null),
        &["accessToken", "access_token"],
    )
    .is_none()
    {
        return Ok(AuthorizationResult {
            state: AuthorizationState::Waiting,
        });
    }
    // Construct headers only from the validated token/domain. UID is fetched from
    // the official profile before any account is admitted to the vault.
    let temporary = parse_account(
        &token,
        &serde_json::json!({"code":0,"data":{"uid":"authorization-pending"}}),
    )?;
    let profile = send_flow(
        session,
        transport,
        id,
        Endpoint::Profile,
        Some(&temporary),
        Some(&flow.state),
        now,
    )
    .await?;
    let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
    // Cancel does not wait for HTTP or the sync lock. Serialize its decision
    // against the final synchronous vault write, not against network latency.
    let mut pending = flows().lock().map_err(|_| Failure::Busy)?;
    let current = pending.get(id).ok_or(Failure::ExpiredAuthorization)?;
    if current.epoch != session.lifecycle_epoch()
        || current.deadline <= std::time::Instant::now()
        || current.root != session.root()
        || current.binding != store::binding(session)?
    {
        return Err(Failure::ExpiredAuthorization);
    }
    // The token may be ready before the official profile. Keep the same flow
    // within its original deadline; cancellation/retirement above still wins.
    if checkin::code(&profile) == Some(12151) {
        return Ok(AuthorizationResult {
            state: AuthorizationState::Waiting,
        });
    }
    let account = parse_account(&token, &profile)?;
    let mut rows = store::load(session, &binding)?;
    let account_id = account.id(&binding.vault_id);
    if let Some(row) = rows
        .iter_mut()
        .find(|r| r.account.id(&binding.vault_id) == account_id)
    {
        row.generation = row
            .generation
            .checked_add(1)
            .ok_or(Failure::StorageUnavailable)?;
        row.auto_claim_revision = row
            .auto_claim_revision
            .checked_add(1)
            .ok_or(Failure::StorageUnavailable)?;
        row.auto_claim_enabled = false;
        row.auto_claim_credential = None;
        // Never erase an unresolved claim or its attempted day during reauthorization.
        row.account = account;
        // This is the legacy check-in grant; never revive a prior inference grant.
        row.inference = None;
    } else {
        rows.push(SavedAccount::new(account));
    }
    store::save(session, &binding, rows)?;
    pending.remove(id);
    Ok(AuthorizationResult {
        state: AuthorizationState::Saved,
    })
}

/// Retire only this opaque flow. Cancellation after a completed save is a no-op,
/// not an account deletion. It must remain available while finish awaits HTTP.
pub(crate) fn cancel(session: &SecretSession, id: &str) -> Result<(), Failure> {
    let mut pending = flows().lock().map_err(|_| Failure::Busy)?;
    if let Some(flow) = pending.get(id) {
        if flow.root != session.root() || flow.binding != store::binding(session)? {
            return Err(Failure::UnsupportedContext);
        }
    }
    pending.remove(id);
    Ok(())
}
