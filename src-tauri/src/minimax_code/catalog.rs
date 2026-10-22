use crate::secrets::session::SecretSession;
use serde::{Deserialize, Serialize};

pub(crate) const FILE: &str = crate::secrets::owned_file::MINIMAX_CODE_FILE;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Region {
    Cn,
    Global,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Status {
    Unavailable,
    Claimable,
    AlreadyClaimed,
    Claimed,
    Unknown,
    NeedsLogin,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AttemptStatus {
    Pending,
    Unknown,
    Claimed,
    AlreadyClaimed,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Failure {
    StorageUnavailable,
    Busy,
    NeedsLogin,
    UnsupportedContext,
    StaleIdentity,
    InvalidResponse,
    NetworkUnavailable,
    ExpiredAuthorization,
    AuthorizationCompleted,
    NotClaimable,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Operation {
    pub attempt_id: String,
    pub started_at: i64,
    pub day: String,
    pub bound_auth_generation: u64,
    pub claim_id: Option<String>,
    pub status: AttemptStatus,
    pub credit_amount: Option<f64>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountView {
    pub auto_claim_enabled: bool,
    pub auto_claim_revision: u64,
    pub auto_claim_failure: Option<Failure>,
    pub entitlement: Option<super::entitlement::Entitlement>,
    pub id: String,
    pub label: String,
    pub region: Region,
    pub build_env: String,
    pub real_user_id: String,
    pub auth_generation: u64,
    pub credential_version: u64,
    pub authorized: bool,
    pub status: Status,
    pub can_claim: bool,
    pub can_refresh: bool,
    pub can_recheck: bool,
    pub can_retire: bool,
    pub operation: Option<Operation>,
    pub last_checked_at: Option<i64>,
    pub credit_amount: Option<f64>,
    pub last_observed_status: Option<Status>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Account {
    #[serde(default)]
    pub auto_claim_enabled: bool,
    #[serde(default)]
    pub auto_claim_revision: u64,
    #[serde(default)]
    pub auto_claim_failure: Option<Failure>,
    pub id: String,
    pub label: String,
    pub region: Region,
    pub build_env: String,
    pub real_user_id: String,
    pub auth_generation: u64,
    pub credential_version: u64,
    pub credentials: Option<Credentials>,
    pub status: Status,
    pub operation: Option<Operation>,
    pub last_checked_at: Option<i64>,
    pub last_observed_status: Option<Status>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Document {
    pub schema_version: u32,
    pub revision: u64,
    pub accounts: Vec<Account>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            accounts: vec![],
        }
    }
}

use crate::{
    config_file_io,
    secrets::{owned_file::OwnedFile, VaultMetadata},
    zcode_file_lock::FileLock,
};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, time::Duration};
use zeroize::{Zeroize, Zeroizing};
impl Drop for Credentials {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Binding {
    pub vault: VaultMetadata,
    pub session_epoch: u64,
    pub process_epoch: u64,
}
impl Binding {
    fn capture(
        session: &SecretSession,
        vault: &crate::secrets::VaultContext,
        runtime: &super::Runtime,
    ) -> Self {
        Self {
            vault: vault.metadata().clone(),
            session_epoch: session.lifecycle_epoch(),
            process_epoch: runtime.epoch,
        }
    }
}
#[derive(Clone)]
pub(super) struct Snapshot {
    pub binding: Binding,
    pub account: Account,
}
pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub(super) fn day(now: i64) -> String {
    chrono::DateTime::from_timestamp_millis(now)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}
pub(super) fn account_id(vault_id: &str, region: Region, uid: &str) -> String {
    let bytes =
        serde_json::to_vec(&(vault_id, region, "prod", uid)).expect("string tuple serializes");
    hex::encode(Sha256::digest(bytes))
}
impl Account {
    pub(super) fn unresolved(&self) -> bool {
        self.operation
            .as_ref()
            .is_some_and(|o| matches!(o.status, AttemptStatus::Pending | AttemptStatus::Unknown))
    }
    pub(super) fn view(&self) -> AccountView {
        let authorized = self.credentials.is_some();
        let status = if self.unresolved() {
            Status::Unknown
        } else if !authorized {
            Status::NeedsLogin
        } else {
            self.status
        };
        AccountView {
            auto_claim_enabled: self.auto_claim_enabled,
            auto_claim_revision: self.auto_claim_revision,
            auto_claim_failure: self.auto_claim_failure,
            entitlement: None,
            id: self.id.clone(),
            label: self.label.clone(),
            region: self.region,
            build_env: self.build_env.clone(),
            real_user_id: self.real_user_id.clone(),
            auth_generation: self.auth_generation,
            credential_version: self.credential_version,
            authorized,
            status,
            can_claim: authorized && status == Status::Claimable,
            can_refresh: authorized,
            can_recheck: authorized && self.unresolved(),
            can_retire: authorized,
            operation: self.operation.clone(),
            last_checked_at: self.last_checked_at,
            credit_amount: self.operation.as_ref().and_then(|o| o.credit_amount),
            last_observed_status: self.last_observed_status,
        }
    }
}
pub(super) fn same_account(current: &Account, expected: &Account) -> bool {
    current.id == expected.id
        && current.region == expected.region
        && current.build_env == expected.build_env
        && current.real_user_id == expected.real_user_id
        && current.auth_generation == expected.auth_generation
        && current.credentials.is_some()
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
pub(super) fn validate_credentials(credentials: &Credentials) -> Result<(), Failure> {
    if !valid_text(&credentials.access_token, 16384)
        || !valid_text(&credentials.refresh_token, 16384)
        || credentials.expires_at <= 0
        || reqwest::header::HeaderValue::from_str(&format!("Bearer {}", credentials.access_token))
            .is_err()
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(())
}
fn validate(document: &Document) -> Result<(), Failure> {
    if document.schema_version != 1 || document.accounts.len() > 200 {
        return Err(Failure::StorageUnavailable);
    }
    let mut ids = HashSet::new();
    let mut identities = HashSet::new();
    for a in &document.accounts {
        if !valid_text(&a.id, 256)
            || !valid_text(&a.real_user_id, 256)
            || !valid_text(&a.label, 256)
            || a.build_env != "prod"
            || a.auth_generation == 0
            || a.credential_version == 0
            || !ids.insert(&a.id)
            || !identities.insert((a.region as u8, &a.real_user_id))
        {
            return Err(Failure::StorageUnavailable);
        }
        if a.last_observed_status.is_some_and(|s| {
            !matches!(
                s,
                Status::Claimable | Status::AlreadyClaimed | Status::Unavailable
            )
        }) {
            return Err(Failure::StorageUnavailable);
        }
        if let Some(c) = &a.credentials {
            validate_credentials(c).map_err(|_| Failure::StorageUnavailable)?;
        }
        if let Some(o) = &a.operation {
            if !valid_text(&o.attempt_id, 128)
                || o.started_at <= 0
                || o.bound_auth_generation == 0
                || o.bound_auth_generation > a.auth_generation
                || o.claim_id.as_ref().is_some_and(|id| !valid_text(id, 1024))
                || chrono::NaiveDate::parse_from_str(&o.day, "%Y-%m-%d").is_err()
                || o.credit_amount.is_some_and(|n| !n.is_finite() || n < 0.0)
            {
                return Err(Failure::StorageUnavailable);
            }
        }
    }
    Ok(())
}
/// Lock order: existing sync owner -> session read/key generation -> existing
/// recoverable file lock -> Runtime -> optional flows lock. Only the first
/// nonblocking network poll may occur here; no network future is awaited.
/// The closure is synchronous; cancellation cannot split mutation and readback.
pub(super) async fn access<R>(
    session: &SecretSession,
    expected: Option<&Binding>,
    f: impl FnOnce(&mut Document, &Binding) -> Result<(R, bool), Failure>,
) -> Result<R, Failure> {
    access_after(session, expected, f, |_| Ok(())).await
}
pub(super) async fn access_after<R>(
    session: &SecretSession,
    expected: Option<&Binding>,
    f: impl FnOnce(&mut Document, &Binding) -> Result<(R, bool), Failure>,
    after: impl FnOnce(&R) -> Result<(), Failure>,
) -> Result<R, Failure> {
    let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
    let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
    let file = OwnedFile::registered(FILE).map_err(|_| Failure::StorageUnavailable)?;
    config_file_io::ensure_private_directory(session.root())
        .map_err(|_| Failure::StorageUnavailable)?;
    let path = session.root().join(FILE);
    let _file = FileLock::acquire_recoverable(&path, Duration::from_millis(50))
        .map_err(|_| Failure::Busy)?;
    let runtime = super::admit()?;
    let binding = Binding::capture(session, &vault, &runtime);
    if expected.is_some_and(|e| e != &binding) {
        return Err(Failure::StaleIdentity);
    }
    let before = config_file_io::read_regular_file(&path, 4 * 1024 * 1024)
        .map_err(|_| Failure::StorageUnavailable)?;
    let mut document = match &before {
        None => Document::default(),
        Some(bytes) => {
            let plain = file
                .decode(&vault, bytes)
                .map_err(|_| Failure::StorageUnavailable)?;
            serde_json::from_slice(&plain).map_err(|_| Failure::StorageUnavailable)?
        }
    };
    validate(&document)?;
    let (result, dirty) = f(&mut document, &binding)?;
    if dirty {
        validate(&document)?;
        document.revision = document
            .revision
            .checked_add(1)
            .ok_or(Failure::StorageUnavailable)?;
        let plain =
            Zeroizing::new(serde_json::to_vec(&document).map_err(|_| Failure::StorageUnavailable)?);
        let encrypted = file
            .encode(&vault, &plain)
            .map_err(|_| Failure::StorageUnavailable)?;
        if encrypted.len() > 4 * 1024 * 1024
            || config_file_io::read_regular_file(&path, 4 * 1024 * 1024)
                .map_err(|_| Failure::StorageUnavailable)?
                != before
        {
            return Err(Failure::StorageUnavailable);
        }
        // Check epochs again under the same key guard immediately before publication.
        if Binding::capture(session, &vault, &runtime) != binding {
            return Err(Failure::StaleIdentity);
        }
        #[cfg(test)]
        super::tests::before_durable(session.root());
        config_file_io::write_durable(&path, &encrypted)
            .map_err(|_| Failure::StorageUnavailable)?;
        let after = config_file_io::read_regular_file(&path, 4 * 1024 * 1024)
            .map_err(|_| Failure::StorageUnavailable)?
            .ok_or(Failure::StorageUnavailable)?;
        #[cfg(test)]
        let after = super::tests::fault_readback(session.root(), after);
        if after != encrypted
            || file
                .decode(&vault, &after)
                .map_err(|_| Failure::StorageUnavailable)?
                .as_slice()
                != plain.as_slice()
        {
            return Err(Failure::StorageUnavailable);
        }
    }
    after(&result)?;
    Ok(result)
}
pub(super) async fn list(session: &SecretSession) -> Result<Vec<AccountView>, Failure> {
    access(session, None, |d, _| {
        Ok((d.accounts.iter().map(Account::view).collect(), false))
    })
    .await
}
pub(super) async fn snapshot(session: &SecretSession, id: &str) -> Result<Snapshot, Failure> {
    access(session, None, |d, b| {
        let account = d
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or(Failure::UnsupportedContext)?;
        if account.credentials.is_none() {
            return Err(Failure::NeedsLogin);
        };
        Ok((
            Snapshot {
                binding: b.clone(),
                account: account.clone(),
            },
            false,
        ))
    })
    .await
}
pub(super) fn matched<'a>(
    d: &'a mut Document,
    snapshot: &Snapshot,
) -> Result<&'a mut Account, Failure> {
    d.accounts
        .iter_mut()
        .find(|a| same_account(a, &snapshot.account))
        .ok_or(Failure::StaleIdentity)
}

/// Dispatch admission is the creation and first poll of the transport future
/// under the same identity/Runtime fence as retirement and shutdown. This is not
/// a claim that bytes are already on wire. Pending network IO is awaited only
/// after every storage/key/Runtime guard has been dropped.
pub(super) enum DispatchFailure {
    NotDispatched(Failure),
    OutcomeUnknown(Failure),
}
impl DispatchFailure {
    pub(super) fn failure(self) -> Failure {
        match self {
            Self::NotDispatched(f) | Self::OutcomeUnknown(f) => f,
        }
    }
}
pub(super) async fn dispatch<T: super::http::Transport>(
    session: &SecretSession,
    binding: &Binding,
    transport: &T,
    request: super::http::Request,
    validate: impl FnOnce(&mut Document, &Binding) -> Result<(), Failure>,
) -> Result<super::http::Response, Failure> {
    dispatch_classified(session, binding, transport, request, validate)
        .await
        .map_err(DispatchFailure::failure)
}
pub(super) async fn dispatch_classified<T: super::http::Transport>(
    session: &SecretSession,
    binding: &Binding,
    transport: &T,
    request: super::http::Request,
    validate: impl FnOnce(&mut Document, &Binding) -> Result<(), Failure>,
) -> Result<super::http::Response, DispatchFailure> {
    #[cfg(test)]
    super::tests::before_dispatch(session.root(), request.endpoint).await;
    let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
    let mut dispatched = false;
    let mut pending = None;
    let mut ready = None;
    access(session, Some(binding), |document, current| {
        validate(document, current)?;
        dispatched = true;
        let mut future = transport.request(request);
        match future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            std::task::Poll::Ready(response) => ready = Some(response),
            std::task::Poll::Pending => pending = Some(future),
        }
        Ok(((), false))
    })
    .await
    .map_err(|failure| {
        if dispatched {
            DispatchFailure::OutcomeUnknown(failure)
        } else {
            DispatchFailure::NotDispatched(failure)
        }
    })?;
    if let Some(response) = ready {
        response.map_err(DispatchFailure::OutcomeUnknown)
    } else {
        pending
            .ok_or(DispatchFailure::OutcomeUnknown(Failure::StorageUnavailable))?
            .await
            .map_err(DispatchFailure::OutcomeUnknown)
    }
}
