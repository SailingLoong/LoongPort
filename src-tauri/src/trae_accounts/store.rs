//! One encrypted document, using the original SecretSession/OwnedFile lifecycle.
use crate::secrets::{session::SecretSession, VaultMetadata};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::sync::Arc;
use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};
use zeroize::Zeroize;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Realm {
    Cn,
    Sg,
    Us,
}
impl Realm {
    pub(crate) fn api(self) -> &'static str {
        match self {
            Self::Cn => "https://trae-api-cn.mchost.guru",
            Self::Sg => "https://coresg-normal.trae.ai",
            Self::Us => "https://coreva-normal.trae.ai",
        }
    }
    pub(super) fn auth(self) -> &'static str {
        match self {
            Self::Cn => "https://api.trae.cn",
            Self::Sg | Self::Us => "https://growsg-normal.trae.ai",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Storage,
    PublicationUnknown,
    Busy,
    Retired,
    Unsupported,
    Invalid,
    #[cfg(test)]
    Network,
    NeedsLogin,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Device {
    // Original selected desktop host. No default or legacy token-host migration.
    pub auth_host: String,
    // Only a newly validated native CN Code import may admit the CN consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_package: Option<String>,
    pub uid: String,
    pub id: String,
    pub machine: String,
    pub client: String,
    pub version: String,
    pub version_code: String,
    pub os: String,
    pub os_version: String,
    pub source_digest: String,
    pub private_key: String,
    pub public_key: String,
}
impl Drop for Device {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Grant {
    pub access: String,
    pub refresh: String,
    pub expires: i64,
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.access.zeroize();
        self.refresh.zeroize();
    }
}
/// A receipt outcome, not a credit-balance assertion. The existing checkin_day
/// remains the sole calendar fact; an unresolved attempt survives every new day.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CheckinAttemptStatus {
    Pending,
    Unknown,
    Accepted,
    Ineligible,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CheckinAttempt {
    pub id: String,
    pub generation: u64,
    pub version: u64,
    pub status: CheckinAttemptStatus,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Account {
    pub id: String,
    pub uid: String,
    pub realm: Realm,
    pub generation: u64,
    pub version: u64,
    pub device: Device,
    pub grant: Option<Grant>,
    pub refresh_pending: bool,
    #[serde(default)]
    pub checkin_day: Option<i64>,
    #[serde(default)]
    pub checkin_attempt: Option<CheckinAttempt>,
    #[serde(default)]
    pub auto_checkin_enabled: bool,
    #[serde(default)]
    pub auto_checkin_revision: u64,
    pub imported_refresh_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Document {
    pub schema: u32,
    pub revision: u64,
    pub accounts: Vec<Account>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            schema: 1,
            revision: 0,
            accounts: vec![],
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Binding {
    pub vault: VaultMetadata,
    pub epoch: u64,
    pub runtime: u64,
}
#[derive(Clone)]
pub(super) struct Snapshot {
    pub binding: Binding,
    pub account: Account,
}
#[derive(Default)]
struct Runtime {
    epoch: u64,
    stopped: bool,
}
fn runtime() -> &'static Mutex<Runtime> {
    static R: OnceLock<Mutex<Runtime>> = OnceLock::new();
    R.get_or_init(Default::default)
}
pub(crate) fn shutdown() {
    if let Ok(mut r) = runtime().lock() {
        r.epoch = r.epoch.saturating_add(1);
        r.stopped = true;
    }
}
pub(super) fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub(super) fn text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn validate(d: &Document) -> Result<(), Failure> {
    if d.schema != 1 || d.accounts.len() > 200 {
        return Err(Failure::Storage);
    }
    let mut ids = std::collections::HashSet::new();
    for a in &d.accounts {
        if !ids.insert(&a.id)
            || !text(&a.id, 256)
            || !text(&a.uid, 256)
            || a.uid != a.device.uid
            || a.device.auth_host != a.realm.auth()
            || a.generation == 0
            || a.version == 0
            || !text(&a.device.id, 256)
            || !text(&a.device.machine, 256)
            || a.device.source_digest.len() != 64
            || a.imported_refresh_hash.len() != 64
        {
            return Err(Failure::Storage);
        }
        if a.checkin_attempt.as_ref().is_some_and(|op| {
            a.checkin_day.is_none()
                || !text(&op.id, 128)
                || op.generation == 0
                || op.generation > a.generation
                || op.version == 0
                || op.version > a.version
        }) {
            return Err(Failure::Storage);
        }
        if let Some(g) = &a.grant {
            if !text(&g.access, 16384) || !text(&g.refresh, 16384) || g.expires <= 0 {
                return Err(Failure::Storage);
            }
        }
    }
    Ok(())
}
/// Original lock order: sync owner -> key/session -> file -> platform runtime.
/// No network IO is awaited while these guards are held.
pub(super) async fn access<R>(
    session: &SecretSession,
    expected: Option<&Binding>,
    f: impl FnOnce(&mut Document, &Binding) -> Result<(R, bool), Failure>,
) -> Result<R, Failure> {
    use crate::{config_file_io as io, secrets::owned_file::OwnedFile, zcode_file_lock::FileLock};
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    let vault = session.read().map_err(|_| Failure::Storage)?;
    let file = OwnedFile::registered(crate::secrets::owned_file::TRAE_FILE)
        .map_err(|_| Failure::Storage)?;
    io::ensure_private_directory(session.root()).map_err(|_| Failure::Storage)?;
    let path = session.root().join(crate::secrets::owned_file::TRAE_FILE);
    let _lock = FileLock::acquire_recoverable(&path, Duration::from_millis(50))
        .map_err(|_| Failure::Busy)?;
    let r = runtime().lock().map_err(|_| Failure::Storage)?;
    if r.stopped {
        return Err(Failure::Retired);
    }
    let binding = Binding {
        vault: vault.metadata().clone(),
        epoch: session.lifecycle_epoch(),
        runtime: r.epoch,
    };
    if expected.is_some_and(|b| b != &binding) {
        return Err(Failure::Retired);
    }
    let before = io::read_regular_file(&path, 4 * 1024 * 1024).map_err(|_| Failure::Storage)?;
    let mut doc = match &before {
        None => Document::default(),
        Some(raw) => {
            serde_json::from_slice(&file.decode(&vault, raw).map_err(|_| Failure::Storage)?)
                .map_err(|_| Failure::Storage)?
        }
    };
    validate(&doc)?;
    let (out, dirty) = f(&mut doc, &binding)?;
    if dirty {
        validate(&doc)?;
        doc.revision = doc.revision.checked_add(1).ok_or(Failure::Storage)?;
        let plain =
            zeroize::Zeroizing::new(serde_json::to_vec(&doc).map_err(|_| Failure::Storage)?);
        let sealed = file.encode(&vault, &plain).map_err(|_| Failure::Storage)?;
        if sealed.len() > 4 * 1024 * 1024
            || io::read_regular_file(&path, 4 * 1024 * 1024).map_err(|_| Failure::Storage)?
                != before
            || session.lifecycle_epoch() != binding.epoch
        {
            return Err(Failure::Retired);
        }
        #[cfg(test)]
        let publication = super::tests::durable_write(&path, &sealed);
        #[cfg(not(test))]
        let publication = io::write_durable(&path, &sealed);
        if publication.is_err() {
            // Original writer can fail BEFORE rename or AFTER publication.
            // Only an exact unchanged destination establishes non-publication.
            return Err(match io::read_regular_file(&path, 4 * 1024 * 1024) {
                Ok(current) if current == before => Failure::Storage,
                _ => Failure::PublicationUnknown,
            });
        }
        let after = io::read_regular_file(&path, 4 * 1024 * 1024)
            .map_err(|_| Failure::PublicationUnknown)?
            .ok_or(Failure::PublicationUnknown)?;
        #[cfg(test)]
        let after = super::tests::readback(&path, after);
        if after != sealed
            || file
                .decode(&vault, &after)
                .map_err(|_| Failure::PublicationUnknown)?
                .as_slice()
                != plain.as_slice()
        {
            return Err(Failure::PublicationUnknown);
        }
    }
    Ok(out)
}
pub(super) async fn snapshot(session: &SecretSession, id: &str) -> Result<Snapshot, Failure> {
    access(session, None, |d, b| {
        let a = d
            .accounts
            .iter()
            .find(|a| a.id == id && a.grant.is_some() && !a.refresh_pending)
            .ok_or(Failure::NeedsLogin)?;
        Ok((
            Snapshot {
                binding: b.clone(),
                account: a.clone(),
            },
            false,
        ))
    })
    .await
}
pub(super) fn matched<'a>(d: &'a mut Document, s: &Snapshot) -> Result<&'a mut Account, Failure> {
    d.accounts
        .iter_mut()
        .find(|a| {
            a.id == s.account.id
                && a.uid == s.account.uid
                && a.realm == s.account.realm
                && a.generation == s.account.generation
                && a.device == s.account.device
                && a.grant.is_some()
        })
        .ok_or(Failure::Retired)
}
pub(crate) async fn retire(session: &SecretSession, id: &str) -> Result<(), Failure> {
    access(session, None, |d, _| {
        let a = d
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(Failure::Retired)?;
        a.generation = a.generation.checked_add(1).ok_or(Failure::Storage)?;
        a.auto_checkin_enabled = false;
        a.auto_checkin_revision = a
            .auto_checkin_revision
            .checked_add(1)
            .ok_or(Failure::Storage)?;
        a.grant = None;
        a.refresh_pending = false;
        Ok(((), true))
    })
    .await
}
/// Read-only local persistence status. Stored does not assert entitlement,
/// upstream availability, or resolve the pending real desktop identity contract.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AccountStatus {
    Stored,
    RefreshPending,
    Retired,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AccountView {
    pub id: String,
    pub realm: Realm,
    pub status: AccountStatus,
    /// Source adapter eligibility only; never a live inference acceptance claim.
    #[serde(rename = "nativeSourceSupported")]
    pub native_source_supported: bool,
    pub generation: u64,
    #[serde(rename = "autoCheckinEnabled")]
    pub auto_checkin_enabled: bool,
    #[serde(rename = "autoCheckinRevision")]
    pub auto_checkin_revision: u64,
    #[serde(rename = "canSetAutomatic")]
    pub can_set_automatic: bool,
    #[serde(rename = "checkinAttemptStatus")]
    pub checkin_attempt_status: Option<CheckinAttemptStatus>,
}
impl Account {
    pub(super) fn checkin_attempt_status(&self) -> Option<CheckinAttemptStatus> {
        self.checkin_attempt
            .as_ref()
            .map(|a| a.status)
            .or(self.checkin_day.map(|_| CheckinAttemptStatus::Unknown))
    }
    pub(super) fn checkin_unresolved(&self) -> bool {
        matches!(
            self.checkin_attempt_status(),
            Some(CheckinAttemptStatus::Pending | CheckinAttemptStatus::Unknown)
        )
    }
    pub(super) fn can_set_automatic(&self) -> bool {
        self.realm == Realm::Cn
            && native_source_supported(self)
            && self.grant.is_some()
            && !self.refresh_pending
    }
    pub(super) fn view(&self) -> AccountView {
        AccountView {
            id: self.id.clone(),
            realm: self.realm,
            generation: self.generation,
            native_source_supported: native_source_supported(self),
            status: if self.grant.is_none() {
                AccountStatus::Retired
            } else if self.refresh_pending {
                AccountStatus::RefreshPending
            } else {
                AccountStatus::Stored
            },
            auto_checkin_enabled: self.auto_checkin_enabled,
            auto_checkin_revision: self.auto_checkin_revision,
            can_set_automatic: self.can_set_automatic(),
            checkin_attempt_status: self.checkin_attempt_status(),
        }
    }
}
pub(super) fn native_source_supported(a: &Account) -> bool {
    a.device.auth_host == a.realm.auth()
        && a.device.client == "ono9krqynydwx5"
        && (a.realm != Realm::Cn || a.device.native_package.as_deref() == Some("TRAE_CN"))
}
pub(crate) async fn list(session: &SecretSession) -> Result<Vec<AccountView>, Failure> {
    access(session, None, |d, _| {
        let views = d.accounts.iter().map(Account::view).collect();
        Ok((views, false))
    })
    .await
}
#[cfg(test)]
pub(crate) async fn lookup(
    session: &SecretSession,
    id: &str,
    realm: Realm,
) -> Result<Option<AccountView>, Failure> {
    Ok(list(session)
        .await?
        .into_iter()
        .find(|a| a.id == id && a.realm == realm))
}
#[cfg(test)]
pub(super) async fn fixture_guard() -> tokio::sync::OwnedMutexGuard<()> {
    static S: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    let g = S
        .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
        .lock_owned()
        .await;
    let mut r = runtime().lock().unwrap();
    r.epoch += 1;
    r.stopped = false;
    g
}
