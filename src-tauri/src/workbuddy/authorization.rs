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
pub(crate) async fn begin<T: Transport>(
    session: &SecretSession,
    transport: &T,
    now: i64,
) -> Result<Login, Failure> {
    let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
    let binding = store::binding(session)?;
    let result = transport
        .request(Endpoint::AuthState, None, None, now)
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
    if store::binding(session)? != binding {
        return Err(Failure::StorageUnavailable);
    }
    let mut map = flows().lock().map_err(|_| Failure::Busy)?;
    map.retain(|_, f| f.expires > now);
    if map.len() >= 64 {
        return Err(Failure::Busy);
    }
    let flow_id = uuid::Uuid::new_v4().to_string();
    map.insert(
        flow_id.clone(),
        Flow {
            state: zeroize::Zeroizing::new(state.into()),
            root: session.root().into(),
            binding,
            expires: now + 600000,
        },
    );
    Ok(Login {
        flow_id,
        verification_uri,
    })
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
        if f.expires <= now {
            map.remove(id);
            return Err(Failure::ExpiredAuthorization);
        }
        f.clone()
    };
    let token = transport
        .request(Endpoint::AuthToken, None, Some(&flow.state), now)
        .await?;
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
    let profile = transport
        .request(Endpoint::Profile, Some(&temporary), Some(&flow.state), now)
        .await?;
    let account = parse_account(&token, &profile)?;
    let mut rows = store::load(session, &binding)?;
    let account_id = account.id(&binding.vault_id);
    if let Some(row) = rows
        .iter_mut()
        .find(|r| r.account.id(&binding.vault_id) == account_id)
    {
        row.account = account;
    } else {
        rows.push(SavedAccount::new(account));
    }
    store::save(session, &binding, rows)?;
    flows().lock().map_err(|_| Failure::Busy)?.remove(id);
    Ok(AuthorizationResult {
        state: AuthorizationState::Saved,
    })
}
