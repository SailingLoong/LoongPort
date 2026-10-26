//! Wire reference: MiniMax-AI/minimax-code 9a935605956c4500e9cb415f29258e2887e97817.
//! Original MIT notice: licenses/minimax-code-MIT.txt.
//! Official mcode-public device grant; no native-session or API-key imports.
//! Protocol facts: MiniMax-AI/minimax-code 9a935605 (MIT), oauth-core and
//! tui/account/matrix-account-client. Verified profile ID is never a JWT subject.
use super::{
    catalog::{self, *},
    http::{Endpoint, Request, Response, Transport},
};
use crate::secrets::session::SecretSession;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
use zeroize::Zeroizing;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Login {
    pub flow_id: String,
    pub verification_uri: String,
    pub user_code: String,
    pub expires_at: i64,
    pub interval_seconds: f64,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AuthorizationState {
    Waiting,
    Saved,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorizationResult {
    pub state: AuthorizationState,
    pub account: Option<AccountView>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlowPhase {
    Pending,
    Publishing,
    Completed,
}
#[derive(Clone)]
struct Flow {
    root: PathBuf,
    binding: Binding,
    identity_generations: HashMap<String, u64>,
    region: Region,
    verifier: Zeroizing<String>,
    device_code: Zeroizing<String>,
    user_code: String,
    user_code_poll: bool,
    expires_at: i64,
    interval_ms: i64,
    next_poll_at: i64,
    polling: bool,
    phase: FlowPhase,
}
fn flows() -> &'static Mutex<HashMap<String, Flow>> {
    static FLOWS: OnceLock<Mutex<HashMap<String, Flow>>> = OnceLock::new();
    FLOWS.get_or_init(Default::default)
}
pub(super) fn clear_flows() {
    if let Ok(mut flows) = flows().lock() {
        flows.clear();
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 16384 && !s.chars().any(char::is_control))
}
fn positive(v: &Value, key: &str) -> Option<f64> {
    v.get(key)?.as_f64().filter(|n| n.is_finite() && *n > 0.0)
}
fn oauth_body(response: &Response) -> Result<&Value, Failure> {
    if !(200..300).contains(&response.status) || response.body.get("error").is_some() {
        return Err(Failure::NeedsLogin);
    };
    if !response.body.is_object() {
        return Err(Failure::InvalidResponse);
    };
    Ok(&response.body)
}
fn token(response: &Response, previous: Option<&str>) -> Result<Credentials, Failure> {
    let b = oauth_body(response)?;
    let access = text(b, "access_token").ok_or(Failure::InvalidResponse)?;
    let refresh = text(b, "refresh_token")
        .or(previous)
        .ok_or(Failure::InvalidResponse)?;
    if !text(b, "token_type").is_some_and(|v| v.eq_ignore_ascii_case("bearer")) {
        return Err(Failure::InvalidResponse);
    }
    let claims = access
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|p| serde_json::from_slice::<Value>(&p).ok());
    let scope = b.get("scope").or_else(|| {
        claims
            .as_ref()
            .and_then(|c| c.get("scope").or_else(|| c.get("scp")))
    });
    let scoped = scope.is_some_and(|s| {
        s.as_str()
            .is_some_and(|v| v.split_whitespace().any(|v| v == "agent.default"))
            || s.as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("agent.default")))
    });
    if !scoped {
        return Err(Failure::UnsupportedContext);
    }
    let expires = positive(b, "expires_in")
        .filter(|n| *n <= 31_536_000.0)
        .ok_or(Failure::InvalidResponse)?;
    let credentials = Credentials {
        access_token: access.into(),
        refresh_token: Some(refresh.into()),
        kind: catalog::CredentialKind::DeviceOAuth,
        native_source: None,
        expires_at: now_ms()
            .checked_add((expires * 1000.0) as i64)
            .ok_or(Failure::InvalidResponse)?,
    };
    catalog::validate_credentials(&credentials)?;
    Ok(credentials)
}
fn profile(response: &Response) -> Result<(String, String), Failure> {
    if response.status == 401 || response.status == 403 {
        return Err(Failure::NeedsLogin);
    }
    if !(200..300).contains(&response.status) {
        return Err(Failure::InvalidResponse);
    }
    for pointer in ["/statusInfo/code", "/base_resp/status_code"] {
        if let Some(value) = response.body.pointer(pointer) {
            let code = value.as_i64().ok_or(Failure::InvalidResponse)?;
            if code == 1_000_048 {
                return Err(Failure::NeedsLogin);
            }
            if code != 0 {
                return Err(Failure::InvalidResponse);
            }
        }
    }
    let b = &response.body;
    let p = b
        .pointer("/data/userInfo")
        .or_else(|| b.pointer("/data/user_info"))
        .or_else(|| b.get("userInfo"))
        .or_else(|| b.get("user_info"))
        .ok_or(Failure::InvalidResponse)?;
    let uid = text(p, "realUserID")
        .or_else(|| text(p, "real_user_id"))
        .filter(|v| v.len() <= 256)
        .ok_or(Failure::InvalidResponse)?;
    let label = text(p, "name")
        .or_else(|| text(p, "userName"))
        .or_else(|| text(p, "user_name"))
        .or_else(|| text(p, "email"))
        .unwrap_or(uid)
        .chars()
        .take(160)
        .collect();
    Ok((uid.into(), label))
}
fn verification_url(raw: &str, region: Region) -> Result<String, Failure> {
    let url = url::Url::parse(raw).map_err(|_| Failure::InvalidResponse)?;
    let allowed = match region {
        Region::Cn => ["account.minimax.cn", "agent.minimaxi.com"],
        Region::Global => ["account.minimax.io", "agent.minimax.io"],
    };
    if url.scheme() != "https"
        || !allowed.contains(&url.host_str().unwrap_or(""))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || url.fragment().is_some()
        || url.query_pairs().any(|(k, _)| {
            let k = k.to_ascii_lowercase();
            k.contains("token") || k == "device_code" || k == "code_verifier"
        })
    {
        return Err(Failure::UnsupportedContext);
    }
    Ok(url.to_string())
}
fn public_request(endpoint: Endpoint, region: Region, body: Value) -> Request {
    Request {
        endpoint,
        region,
        access_token: None,
        real_user_id: None,
        body: Some(body),
    }
}
fn active_flow(
    session: &SecretSession,
    id: &str,
    expected: &Flow,
    binding: &Binding,
) -> Result<(), Failure> {
    let map = flows().lock().map_err(|_| Failure::Busy)?;
    let current = map
        .get(id)
        .filter(|f| f.root == session.root() && f.binding == *binding && f.expires_at > now_ms())
        .ok_or(Failure::ExpiredAuthorization)?;
    if current.phase == FlowPhase::Completed {
        return Err(Failure::AuthorizationCompleted);
    }
    if current.phase != FlowPhase::Pending {
        return Err(Failure::ExpiredAuthorization);
    }
    if current.device_code != expected.device_code {
        return Err(Failure::StaleIdentity);
    }
    Ok(())
}
/// One profile-verification owner for independently authorized credentials and
/// future explicitly confirmed native reads. No refresh token is accepted,
/// copied, refreshed or saved here; local/JWT identity claims are not trusted.
/// This result alone cannot authorize persistence: the caller must still check
/// its source/account incarnation and credential ownership at commit time.
pub(super) async fn verify_access_token<T: Transport>(
    session: &SecretSession,
    transport: &T,
    region: Region,
    access_token: &str,
    binding: &Binding,
    validate: impl FnOnce(&mut Document, &Binding) -> Result<(), Failure>,
) -> Result<(String, String), Failure> {
    catalog::validate_access_token(access_token)?;
    let response = catalog::dispatch(
        session,
        binding,
        transport,
        Request {
            endpoint: Endpoint::Profile,
            region,
            access_token: Some(access_token.to_owned()),
            real_user_id: None,
            body: None,
        },
        validate,
    )
    .await?;
    // The transport can complete after vault recovery or process retirement.
    // Never return a verified identity across that lifecycle boundary.
    catalog::access(session, Some(binding), |_, _| {
        Ok((profile(&response)?, false))
    })
    .await
}
pub(crate) async fn begin<T: Transport>(
    session: &SecretSession,
    transport: &T,
    region: Region,
) -> Result<Login, Failure> {
    // Bind an explicit login intent to the identities that existed when it
    // began. A later retirement or reauthorization must not be undone by this
    // old device flow; normal same-identity refresh preserves this generation.
    let (binding, identity_generations) = catalog::access(session, None, |d, b| {
        let generations = d
            .accounts
            .iter()
            .filter(|a| a.region == region)
            .map(|a| (a.real_user_id.clone(), a.auth_generation))
            .collect();
        Ok(((b.clone(), generations), false))
    })
    .await?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| Failure::StorageUnavailable)?;
    let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(random));
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let response=catalog::dispatch(session, &binding, transport, public_request(Endpoint::DeviceCode,region,json!({"client_id":"mcode-public","scope":"agent.default","audience":"agent-backend","code_challenge":challenge,"code_challenge_method":"S256"})), |_, _| Ok(())).await?;
    let b = oauth_body(&response)?;
    let now = now_ms();
    let user_code = text(b, "user_code")
        .filter(|v| v.len() <= 128)
        .ok_or(Failure::InvalidResponse)?
        .to_owned();
    let legacy_expiry = positive(b, "expired_in");
    let device = text(b, "device_code");
    let user_code_poll = device.is_none() && legacy_expiry.is_some();
    let device_code = device
        .or_else(|| user_code_poll.then_some(user_code.as_str()))
        .ok_or(Failure::InvalidResponse)?
        .to_owned();
    let seconds = positive(b, "expires_in")
        .or_else(|| {
            legacy_expiry.map(|n| {
                if n >= 1_000_000_000_000.0 {
                    ((n - now as f64) / 1000.0).ceil()
                } else {
                    n
                }
            })
        })
        .filter(|n| n.is_finite() && *n > 0.0 && *n <= 86400.0)
        .ok_or(Failure::InvalidResponse)?;
    let interval = positive(b, "interval")
        .map(|n| if user_code_poll { n / 1000.0 } else { n })
        .unwrap_or(5.0)
        .clamp(1.0, 300.0);
    let uri = text(b, "verification_uri_complete")
        .or_else(|| text(b, "verification_uri"))
        .or_else(|| text(b, "verification_url"))
        .ok_or(Failure::InvalidResponse)?;
    let verification_uri = verification_url(uri, region)?;
    let expires_at = now + (seconds * 1000.0) as i64;
    let flow_id = uuid::Uuid::new_v4().to_string();
    catalog::access(session, Some(&binding), |_, _| {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        map.retain(|_, f| f.expires_at > now);
        if map.len() >= 64 {
            return Err(Failure::Busy);
        }
        map.insert(
            flow_id.clone(),
            Flow {
                root: session.root().into(),
                binding: binding.clone(),
                identity_generations,
                region,
                verifier,
                device_code: Zeroizing::new(device_code),
                user_code: user_code.clone(),
                user_code_poll,
                expires_at,
                interval_ms: (interval * 1000.0) as i64,
                next_poll_at: now,
                polling: false,
                phase: FlowPhase::Pending,
            },
        );
        Ok(((), false))
    })
    .await?;
    Ok(Login {
        flow_id,
        verification_uri,
        user_code,
        expires_at,
        interval_seconds: interval,
    })
}
struct PollGuard(String);
impl Drop for PollGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = flows().lock() {
            if let Some(flow) = map.get_mut(&self.0) {
                flow.polling = false;
            }
        }
    }
}
pub(crate) async fn poll<T: Transport>(
    session: &SecretSession,
    transport: &T,
    id: &str,
) -> Result<AuthorizationResult, Failure> {
    let flow = {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        let f = map.get_mut(id).ok_or(Failure::ExpiredAuthorization)?;
        if f.root != session.root() || f.expires_at <= now_ms() {
            return Err(Failure::ExpiredAuthorization);
        };
        if f.phase == FlowPhase::Completed {
            return Err(Failure::AuthorizationCompleted);
        }
        if f.phase == FlowPhase::Publishing {
            return Err(Failure::StorageUnavailable);
        }
        if f.polling {
            return Err(Failure::Busy);
        };
        f.polling = true;
        f.clone()
    };
    let _poll = PollGuard(id.into());
    catalog::access(session, Some(&flow.binding), |_, _| Ok(((), false))).await?;
    if flow.next_poll_at > now_ms() {
        return Ok(AuthorizationResult {
            state: AuthorizationState::Waiting,
            account: None,
        });
    }
    let mut body = json!({"grant_type":"urn:ietf:params:oauth:grant-type:device_code","client_id":"mcode-public","code_verifier":flow.verifier.as_str()});
    body[if flow.user_code_poll {
        "user_code"
    } else {
        "device_code"
    }] = Value::String(if flow.user_code_poll {
        flow.user_code.clone()
    } else {
        flow.device_code.to_string()
    });
    let response = catalog::dispatch(
        session,
        &flow.binding,
        transport,
        public_request(Endpoint::Token, flow.region, body),
        |_, b| active_flow(session, id, &flow, b),
    )
    .await?;
    let polling = response
        .body
        .get("error")
        .or_else(|| response.body.get("status"))
        .and_then(Value::as_str);
    if matches!(
        polling,
        Some("authorization_pending" | "pending" | "slow_down")
    ) {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        let f = map.get_mut(id).ok_or(Failure::ExpiredAuthorization)?;
        if polling == Some("slow_down") {
            f.interval_ms = f.interval_ms.saturating_add(5000)
        };
        f.next_poll_at = now_ms() + f.interval_ms;
        return Ok(AuthorizationResult {
            state: AuthorizationState::Waiting,
            account: None,
        });
    }
    let credentials = token(&response, None)?;
    let (uid, label) = verify_access_token(
        session,
        transport,
        flow.region,
        &credentials.access_token,
        &flow.binding,
        |_, b| active_flow(session, id, &flow, b),
    )
    .await?;
    #[cfg(test)]
    super::tests::before_authorization_commit(session.root()).await;
    let account = catalog::access_after(
        session,
        Some(&flow.binding),
        |d, b| {
            active_flow(session, id, &flow, b)?;
            let current_generation = d
                .accounts
                .iter()
                .find(|a| a.region == flow.region && a.real_user_id == uid)
                .map(|a| a.auth_generation);
            if current_generation != flow.identity_generations.get(&uid).copied() {
                return Err(Failure::StaleIdentity);
            }
            let mut map = flows().lock().map_err(|_| Failure::Busy)?;
            let account_id = catalog::account_id(b.vault.store_id(), flow.region, &uid);
            let i = if let Some(i) = d.accounts.iter().position(|a| {
                a.region == flow.region && a.build_env == "prod" && a.real_user_id == uid
            }) {
                let a = &mut d.accounts[i];
                a.auth_generation = a
                    .auth_generation
                    .checked_add(1)
                    .ok_or(Failure::StorageUnavailable)?;
                a.credential_version = a
                    .credential_version
                    .checked_add(1)
                    .ok_or(Failure::StorageUnavailable)?;
                a.auto_claim_enabled = false;
                a.auto_claim_revision = a
                    .auto_claim_revision
                    .checked_add(1)
                    .ok_or(Failure::StorageUnavailable)?;
                a.auto_claim_failure = None;
                a.credentials = Some(credentials.clone());
                a.label = label.clone();
                if !a.unresolved() {
                    a.status = Status::Unavailable
                }
                i
            } else {
                d.accounts.push(Account {
                    auto_claim_enabled: false,
                    auto_claim_revision: 0,
                    auto_claim_failure: None,
                    id: account_id,
                    label: label.clone(),
                    region: flow.region,
                    build_env: "prod".into(),
                    real_user_id: uid.clone(),
                    auth_generation: 1,
                    credential_version: 1,
                    credentials: Some(credentials.clone()),
                    status: Status::Unavailable,
                    operation: None,
                    last_checked_at: None,
                    last_observed_status: None,
                });
                d.accounts.len() - 1
            };
            let view = d.accounts[i].view();
            // From this point cancellation cannot claim to have revoked this flow,
            // even if publication/readback subsequently fails. Completed is set only
            // after authenticated readback, under the same Runtime fence.
            map.get_mut(id).ok_or(Failure::ExpiredAuthorization)?.phase = FlowPhase::Publishing;
            Ok((view, true))
        },
        |_| {
            flows()
                .lock()
                .map_err(|_| Failure::Busy)?
                .get_mut(id)
                .ok_or(Failure::ExpiredAuthorization)?
                .phase = FlowPhase::Completed;
            Ok(())
        },
    )
    .await?;
    Ok(AuthorizationResult {
        state: AuthorizationState::Saved,
        account: Some(account),
    })
}
pub(crate) async fn cancel(session: &SecretSession, id: &str) -> Result<(), Failure> {
    let _runtime = super::admit()?;
    let mut map = flows().lock().map_err(|_| Failure::Busy)?;
    let flow = map.get(id).ok_or(Failure::ExpiredAuthorization)?;
    if flow.root != session.root() {
        return Err(Failure::UnsupportedContext);
    }
    if flow.phase == FlowPhase::Completed {
        return Err(Failure::AuthorizationCompleted);
    }
    if flow.phase == FlowPhase::Publishing {
        return Err(Failure::StorageUnavailable);
    }
    map.remove(id);
    Ok(())
}
// A flight stores only completion, never another copy of credentials. Its weak
// registry exists only while this attempt has participants. It does not claim
// crash/restart recovery or change later independent retry policy.
struct RefreshFlight {
    binding: Binding,
    auth_generation: u64,
    credential_version: u64,
    result: tokio::sync::watch::Sender<Option<Result<(), Failure>>>,
}
struct RefreshLeader(std::sync::Arc<RefreshFlight>, bool);
impl RefreshLeader {
    fn finish(mut self, result: Result<(), Failure>) {
        self.0.result.send_replace(Some(result));
        self.1 = false;
    }
}
impl Drop for RefreshLeader {
    fn drop(&mut self) {
        if self.1 {
            // A dropped leader cannot prove that an accepted refresh did not
            // rotate the upstream token. Existing waiters must not replay it.
            self.0
                .result
                .send_replace(Some(Err(Failure::NetworkUnavailable)));
        }
    }
}
fn refresh_flight(
    session: &SecretSession,
    snapshot: &Snapshot,
) -> Result<(std::sync::Arc<RefreshFlight>, bool), Failure> {
    type Flights = HashMap<(PathBuf, String), std::sync::Weak<RefreshFlight>>;
    static FLIGHTS: OnceLock<Mutex<Flights>> = OnceLock::new();
    let mut flights = FLIGHTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| Failure::StorageUnavailable)?;
    flights.retain(|_, flight| flight.strong_count() > 0);
    let key = (session.root().to_path_buf(), snapshot.account.id.clone());
    if let Some(flight) = flights.get(&key).and_then(std::sync::Weak::upgrade) {
        if flight.binding == snapshot.binding
            && flight.auth_generation == snapshot.account.auth_generation
            && flight.credential_version == snapshot.account.credential_version
        {
            return Ok((flight, false));
        }
    }
    let (result, _) = tokio::sync::watch::channel(None);
    let flight = std::sync::Arc::new(RefreshFlight {
        binding: snapshot.binding.clone(),
        auth_generation: snapshot.account.auth_generation,
        credential_version: snapshot.account.credential_version,
        result,
    });
    flights.insert(key, std::sync::Arc::downgrade(&flight));
    Ok((flight, true))
}
pub(super) async fn refresh_snapshot<T: Transport>(
    session: &SecretSession,
    transport: &T,
    snapshot: &Snapshot,
) -> Result<Snapshot, Failure> {
    let (flight, leader) = refresh_flight(session, snapshot)?;
    if leader {
        let guard = RefreshLeader(flight, true);
        let result = refresh_snapshot_once(session, transport, snapshot).await;
        guard.finish(result.as_ref().map(|_| ()).map_err(|error| *error));
        return result;
    }
    let mut completion = flight.result.subscribe();
    loop {
        let result = *completion.borrow_and_update();
        if let Some(result) = result {
            result?;
            // Never use an old leader result across revoke/re-login/vault change.
            // Read the original owner, rather than cache a second secret owner.
            return catalog::access(session, Some(&snapshot.binding), |document, binding| {
                let account = catalog::matched(document, snapshot)?;
                account.usable()?;
                if account.credential_version <= snapshot.account.credential_version {
                    return Err(Failure::StaleIdentity);
                }
                Ok((
                    Snapshot {
                        binding: binding.clone(),
                        account: account.clone(),
                    },
                    false,
                ))
            })
            .await;
        }
        completion
            .changed()
            .await
            .map_err(|_| Failure::NetworkUnavailable)?;
    }
}
async fn refresh_snapshot_once<T: Transport>(
    session: &SecretSession,
    transport: &T,
    snapshot: &Snapshot,
) -> Result<Snapshot, Failure> {
    catalog::access(session, Some(&snapshot.binding), |d, _| {
        let a = catalog::matched(d, snapshot)?;
        if a.credential_version != snapshot.account.credential_version {
            return Err(Failure::StaleIdentity);
        };
        Ok(((), false))
    })
    .await?;
    let old = snapshot
        .account
        .credentials
        .as_ref()
        .ok_or(Failure::NeedsLogin)?;
    if old.kind == catalog::CredentialKind::NativeAccessOnly {
        // This path follows an explicit upstream 401. Invalidate only this local
        // access copy, not the source app/grant and not an unresolved claim.
        catalog::access(session, Some(&snapshot.binding), |d, _| {
            let a = catalog::matched(d, snapshot)?;
            if a.credential_version != snapshot.account.credential_version {
                return Err(Failure::StaleIdentity);
            }
            a.credentials
                .as_mut()
                .and_then(|c| c.native_source.as_mut())
                .ok_or(Failure::InvalidResponse)?
                .rejected = true;
            Ok(((), true))
        })
        .await?;
        return Err(Failure::NeedsLogin);
    }
    let refresh_token = old.refresh_token.as_deref().ok_or(Failure::NeedsLogin)?;
    let response=catalog::dispatch(session, &snapshot.binding, transport, public_request(Endpoint::Refresh,snapshot.account.region,json!({"grant_type":"refresh_token","refresh_token":refresh_token,"client_id":"mcode-public","scope":"agent.default","audience":"agent-backend"})), |d, _| {
        let a = catalog::matched(d, snapshot)?;
        if a.credential_version != snapshot.account.credential_version { return Err(Failure::StaleIdentity); }
        Ok(())
    }).await?;
    let credentials = token(&response, Some(refresh_token))?;
    let (uid, label) = verify_access_token(
        session,
        transport,
        snapshot.account.region,
        &credentials.access_token,
        &snapshot.binding,
        |d, _| {
            let a = catalog::matched(d, snapshot)?;
            if a.credential_version != snapshot.account.credential_version {
                return Err(Failure::StaleIdentity);
            }
            Ok(())
        },
    )
    .await?;
    if uid != snapshot.account.real_user_id {
        return Err(Failure::StaleIdentity);
    }
    catalog::access(session, Some(&snapshot.binding), |d, b| {
        let a = catalog::matched(d, snapshot)?;
        if a.credential_version != snapshot.account.credential_version {
            return Err(Failure::StaleIdentity);
        };
        a.credentials = Some(credentials);
        a.label = label;
        a.credential_version = a
            .credential_version
            .checked_add(1)
            .ok_or(Failure::StorageUnavailable)?;
        Ok((
            Snapshot {
                binding: b.clone(),
                account: a.clone(),
            },
            true,
        ))
    })
    .await
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    #[test]
    fn profile_rejects_official_status_info_login_error_even_with_stale_data() {
        let response = Response {
            status: 200,
            body: json!({"statusInfo":{"code":1000048},"base_resp":{"status_code":0},"data":{"userInfo":{"realUserID":"stale-user"}}}),
        };
        assert_eq!(profile(&response), Err(Failure::NeedsLogin));
    }
    #[test]
    fn profile_accepts_official_status_info_envelope_without_base_resp() {
        let response = Response {
            status: 200,
            body: json!({"statusInfo":{"code":0},"data":{"userInfo":{"realUserID":"verified-user","name":"Synthetic"}}}),
        };
        assert_eq!(
            profile(&response),
            Ok(("verified-user".into(), "Synthetic".into()))
        );
    }
}
