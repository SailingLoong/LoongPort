//! Current registered-device proof only; no legacy fallback or client switching.
use super::store::{Device, Failure};
use serde_json::Value;
pub(super) const PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
const PROFILE_PATH: &str = "/cloudide/api/v3/trae/GetUserInfo";
pub(super) fn proof(
    d: &Device,
    refresh: &str,
    timestamp: i64,
    nonce: &str,
) -> Result<Value, Failure> {
    use base64::Engine;
    if timestamp <= 0 || nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Failure::Invalid);
    }
    let key = super::import::key_pair(&d.private_key, &d.public_key)?;
    let message = zeroize::Zeroizing::new(format!(
        "POST\n{PATH}\n{}\n{refresh}\n{timestamp}\n{nonce}",
        d.client
    ));
    let signature = key
        .sign(&ring::rand::SystemRandom::new(), message.as_bytes())
        .map_err(|_| Failure::Invalid)?;
    Ok(
        serde_json::json!({"Signature":base64::engine::general_purpose::STANDARD.encode(signature.as_ref()),"Timestamp":timestamp,"Nonce":nonce}),
    )
}
use super::{
    inference::{self, Request, Transport},
    store::{self, Grant, Snapshot},
};
use crate::{proxy::ProxyError, secrets::session::SecretSession};
use std::{sync::Arc, time::Duration};

fn auth_error() -> ProxyError {
    ProxyError::AuthError("trae.authorization_unavailable".into())
}
fn owner_error(failure: Failure) -> ProxyError {
    if failure == Failure::PublicationUnknown {
        ProxyError::AuthError("trae.publication_unknown".into())
    } else {
        auth_error()
    }
}
fn refresh_lock(
    session: &SecretSession,
    s: &Snapshot,
) -> Result<Arc<tokio::sync::Mutex<()>>, ProxyError> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, Weak};
    static LOCKS: OnceLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let key = format!(
        "{}:{}:{}:{}",
        session.root().display(),
        s.binding.vault.store_id(),
        s.account.id,
        s.account.generation
    );
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| auth_error())?;
    locks.retain(|_, v| v.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    Ok(lock)
}
/// Reconcile a pre-acquired lease after a same-generation refresh. Waiting here
/// also prevents observing another request's durable pending rotation as a grant.
pub(super) async fn synchronize(
    session: &Arc<SecretSession>,
    snapshot: &mut Snapshot,
) -> Result<(), ProxyError> {
    let lock = refresh_lock(session, snapshot)?;
    let _guard = lock.lock().await;
    *snapshot = store::access(session, Some(&snapshot.binding), |d, b| {
        let a = store::matched(d, snapshot)?;
        if a.refresh_pending || a.version < snapshot.account.version {
            return Err(Failure::NeedsLogin);
        }
        Ok((
            Snapshot {
                binding: b.clone(),
                account: a.clone(),
            },
            false,
        ))
    })
    .await
    .map_err(owner_error)?;
    Ok(())
}
pub(super) fn refresh_request(s: &Snapshot, timeout: Duration) -> Result<Request, ProxyError> {
    use ring::rand::SecureRandom;
    let a = &s.account;
    supported_desktop(a)?;
    let g = a.grant.as_ref().ok_or_else(auth_error)?;
    let d = &a.device;
    let mut nonce = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| auth_error())?;
    let proof =
        proof(d, &g.refresh, store::now() / 1000, &hex::encode(nonce)).map_err(|_| auth_error())?;
    if d.client != "ono9krqynydwx5" {
        return Err(auth_error());
    }
    let mut body = serde_json::json!({"ClientID":d.client,"ClientSecret":"","RefreshToken":g.refresh,"DeviceInfo":{"DeviceID":d.id,"MachineID":d.machine,"PlatformCode":"IDE_PC","DeviceType":"PC","ClientVersion":d.version,"DevicePublicKey":d.public_key,"OSInfo":d.os,"OSVersion":d.os_version},"DeviceProof":proof,"IDEVersion":d.version});
    if d.os_version.is_empty() {
        body["DeviceInfo"]
            .as_object_mut()
            .unwrap()
            .remove("OSVersion");
    }
    let mut headers = http::HeaderMap::new();
    inference::header(&mut headers, "content-type", "application/json", false)?;
    inference::header(&mut headers, "accept-encoding", "identity", false)?;
    // Native desktop renewal carries the current access token alongside the
    // original device proof. Do not substitute a different auth scheme/client.
    inference::header(&mut headers, "x-cloudide-token", &g.access, true)?;
    Ok(Request {
        url: format!("{}{PATH}", d.auth_host),
        headers,
        body,
        timeout,
    })
}
pub(super) async fn refresh(
    session: &Arc<SecretSession>,
    snapshot: &mut Snapshot,
    timeout: Duration,
    transport: &impl Transport,
) -> Result<(), ProxyError> {
    let lock = refresh_lock(session, snapshot)?;
    let _guard = lock.lock().await;
    let current = store::access(session, Some(&snapshot.binding), |d, b| {
        let a = store::matched(d, snapshot)?;
        if a.refresh_pending {
            return Err(Failure::NeedsLogin);
        }
        Ok((
            Snapshot {
                binding: b.clone(),
                account: a.clone(),
            },
            false,
        ))
    })
    .await
    .map_err(owner_error)?;
    if current.account.version != snapshot.account.version {
        *snapshot = current;
        return Ok(());
    }
    let request = refresh_request(&current, timeout)?;
    store::access(session, Some(&current.binding), |d, _| {
        let a = store::matched(d, &current)?;
        if a.version != current.account.version || a.refresh_pending {
            return Err(Failure::Retired);
        }
        a.refresh_pending = true;
        Ok(((), true))
    })
    .await
    .map_err(owner_error)?;
    *snapshot = exchange_and_commit(session, &current, request, transport, None).await?;
    Ok(())
}
async fn exchange_and_commit(
    session: &Arc<SecretSession>,
    current: &Snapshot,
    request: Request,
    transport: &impl Transport,
    admission: Option<&super::entry::Admission>,
) -> Result<Snapshot, ProxyError> {
    let timeout = request.timeout;
    let response = inference::admit(session, current, transport, request, true).await?;
    if !response.status().is_success() {
        return Err(auth_error());
    }
    let bytes = response
        .bytes_with_limit(1024 * 1024)
        .await
        .map_err(|_| auth_error())?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| auth_error())?;
    let grant = accepted(current, &body).map_err(|_| auth_error())?;
    verify_profile(session, current, &grant, timeout, transport).await?;
    let committed = store::access(session, Some(&current.binding), |d, b| {
        if let Some(control) = admission {
            control.check()?;
        }
        let a = store::matched(d, current)?;
        if !a.refresh_pending || a.version != current.account.version {
            return Err(Failure::Retired);
        }
        a.version = a.version.checked_add(1).ok_or(Failure::Storage)?;
        a.grant = Some(grant);
        a.refresh_pending = false;
        Ok((
            Snapshot {
                binding: b.clone(),
                account: a.clone(),
            },
            true,
        ))
    })
    .await
    .map_err(owner_error)?;
    Ok(committed)
}
pub(super) fn expiry(v: &Value) -> Result<i64, Failure> {
    let n = v
        .as_i64()
        .or_else(|| v.as_str()?.parse().ok())
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(v.as_str()?)
                .ok()
                .map(|d| d.timestamp_millis())
        })
        .ok_or(Failure::Invalid)?;
    if n <= 0 {
        return Err(Failure::Invalid);
    }
    if n < 100000000000 {
        n.checked_mul(1000).ok_or(Failure::Invalid)
    } else {
        Ok(n)
    }
}
/// Reject unsupported desktop sources before persistence or token transmission.
fn supported_desktop(a: &store::Account) -> Result<(), ProxyError> {
    if a.realm == store::Realm::Cn && a.device.native_package.as_deref() != Some("TRAE_CN") {
        return Err(ProxyError::AuthError(
            "trae.desktop_identity_unverified".into(),
        ));
    }
    if a.device.auth_host != a.realm.auth() || a.device.client != "ono9krqynydwx5" {
        return Err(auth_error());
    }
    Ok(())
}
fn successful_body(body: &Value) -> bool {
    if ["/error", "/Error", "/ResponseMetadata/Error"]
        .iter()
        .any(|p| body.pointer(p).is_some_and(|e| !e.is_null()))
    {
        return false;
    }
    ["code", "Code"].iter().all(|k| {
        body.get(k)
            .is_none_or(|v| v.as_i64() == Some(0) || v.as_str() == Some("0"))
    })
}
/// Device-bound exchange metadata is necessary but not sufficient: fresh identity
/// is obtained separately using the new Token. Desktop JWT claims are not parsed.
pub(super) fn accepted(s: &Snapshot, body: &Value) -> Result<Grant, Failure> {
    if !successful_body(body) {
        return Err(Failure::NeedsLogin);
    }
    let r = body.get("Result").ok_or(Failure::Invalid)?;
    if s.account.realm == store::Realm::Cn {
        // Official CN Code 3.4.4 accepts Result without these receipt fields.
        // Device binding is proved in the signed request; fresh UID/realm is
        // checked before publication. Optional metadata must not contradict it.
        if r.get("DeviceBindStatus")
            .is_some_and(|v| v.as_str() != Some("BOUND"))
            || r.get("BoundDeviceID")
                .is_some_and(|v| v.as_str() != Some(s.account.device.id.as_str()))
        {
            return Err(Failure::Invalid);
        }
    } else if r.get("DeviceBindStatus").and_then(Value::as_str) != Some("BOUND")
        || r.get("BoundDeviceID").and_then(Value::as_str) != Some(s.account.device.id.as_str())
    {
        return Err(Failure::Invalid);
    }
    let access = super::import::value(r, "Token", 16384)?;
    let refresh = super::import::value(r, "RefreshToken", 16384)?;
    if r.get("ClientID")
        .is_some_and(|c| c.as_str() != Some(s.account.device.client.as_str()))
    {
        return Err(Failure::Invalid);
    }
    // Optional standard-exchange metadata cannot contradict the selected owner.
    // It never replaces the normal desktop fresh-profile request below.
    if let Some(info) = r.get("UserInfo") {
        profile_identity(s, info)?;
    }
    let expires = expiry(&r["TokenExpireAt"])?;
    if expires <= store::now() {
        return Err(Failure::NeedsLogin);
    }
    http::HeaderValue::from_str(&access).map_err(|_| Failure::Invalid)?;
    Ok(Grant {
        access,
        refresh,
        expires,
    })
}
fn profile_identity(s: &Snapshot, info: &Value) -> Result<(), Failure> {
    let expected_region = match s.account.realm {
        store::Realm::Cn => "CN",
        store::Realm::Sg => "SG",
        store::Realm::Us => "US",
    };
    if info.get("UserID").and_then(Value::as_str) != Some(s.account.uid.as_str())
        || info
            .get("AIRegion")
            .and_then(Value::as_str)
            .map(str::to_ascii_uppercase)
            .as_deref()
            != Some(expected_region)
        || info
            .get("Host")
            .is_some_and(|h| h.as_str() != Some(s.account.device.auth_host.as_str()))
    {
        return Err(Failure::Invalid);
    }
    Ok(())
}
/// Independently implemented from the public 3.6.2 TRAE_I18N protocol facts.
/// UID comes from the normal same-host profile, authenticated with the new token.
/// AIRegion equality is our stricter consistency check, not an official realm proof.
async fn verify_profile(
    session: &Arc<SecretSession>,
    current: &Snapshot,
    grant: &Grant,
    timeout: Duration,
    transport: &impl Transport,
) -> Result<(), ProxyError> {
    supported_desktop(&current.account)?;
    let d = &current.account.device;
    let mut headers = http::HeaderMap::new();
    inference::header(&mut headers, "content-type", "application/json", false)?;
    inference::header(&mut headers, "accept-encoding", "identity", false)?;
    inference::header(&mut headers, "x-cloudide-token", &grant.access, true)?;
    let request = Request {
        url: format!("{}{PROFILE_PATH}", d.auth_host),
        headers,
        body: serde_json::json!({"ReqSource":"IDE","IDEVersion":d.version}),
        timeout,
    };
    let response = inference::admit(session, current, transport, request, true)
        .await
        .map_err(|_| auth_error())?;
    if !response.status().is_success() {
        return Err(auth_error());
    }
    let bytes = response
        .bytes_with_limit(1024 * 1024)
        .await
        .map_err(|_| auth_error())?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| auth_error())?;
    if !successful_body(&body) {
        return Err(auth_error());
    }
    profile_identity(current, body.get("Result").ok_or_else(auth_error)?).map_err(|_| auth_error())
}
/// Synthetic protocol tests exercise the existing owner without an entry flow.
/// Production activation only enters through the explicit guarded selection flow.
#[cfg(test)]
pub(super) async fn activate_with(
    session: Arc<SecretSession>,
    material: super::import::Material,
    timeout: Duration,
    transport: &impl Transport,
) -> Result<String, ProxyError> {
    activate_inner(session, material, timeout, transport, None, None).await
}
pub(super) async fn activate_guarded(
    session: Arc<SecretSession>,
    material: super::import::Material,
    timeout: Duration,
    transport: &impl Transport,
    binding: &store::Binding,
    admission: &super::entry::Admission,
) -> Result<String, ProxyError> {
    activate_inner(
        session,
        material,
        timeout,
        transport,
        Some(binding),
        Some(admission),
    )
    .await
}
async fn activate_inner(
    session: Arc<SecretSession>,
    material: super::import::Material,
    timeout: Duration,
    transport: &impl Transport,
    expected: Option<&store::Binding>,
    admission: Option<&super::entry::Admission>,
) -> Result<String, ProxyError> {
    use sha2::{Digest, Sha256};
    let mut row = material.account;
    supported_desktop(&row)?;
    let staged = store::access(&session, expected, |d, b| {
        if let Some(control) = admission {
            control.check()?;
        }
        row.id = hex::encode(Sha256::digest(
            serde_json::to_vec(&(b.vault.store_id(), row.realm, &row.uid))
                .map_err(|_| Failure::Invalid)?,
        ));
        row.refresh_pending = true;
        if let Some(previous) = d.accounts.iter_mut().find(|a| a.id == row.id) {
            // A new official login must actually supply a different refresh.
            // Cosmetic storage changes and concurrent imports cannot replay an
            // uncertain original refresh/device attempt.
            if previous.imported_refresh_hash == row.imported_refresh_hash
                || (previous.refresh_pending
                    && previous
                        .grant
                        .as_ref()
                        .zip(row.grant.as_ref())
                        .is_some_and(|(a, b)| a.refresh == b.refresh))
            {
                return Err(Failure::NeedsLogin);
            }
            row.checkin_day = previous.checkin_day;
            row.checkin_attempt = previous.checkin_attempt.clone();
            row.auto_checkin_enabled = false;
            row.auto_checkin_revision = previous
                .auto_checkin_revision
                .checked_add(1)
                .ok_or(Failure::Storage)?;
            row.generation = previous.generation.checked_add(1).ok_or(Failure::Storage)?;
            row.version = previous.version.checked_add(1).ok_or(Failure::Storage)?;
            *previous = row.clone();
        } else {
            row.generation = 1;
            row.version = 1;
            d.accounts.push(row.clone());
        }
        if let Some(control) = admission {
            control.staged(&row)?;
        }
        Ok((
            Snapshot {
                binding: b.clone(),
                account: row,
            },
            true,
        ))
    })
    .await
    .map_err(owner_error)?;
    let request = refresh_request(&staged, timeout)?;
    let committed = exchange_and_commit(&session, &staged, request, transport, admission).await?;
    Ok(committed.account.id)
}
