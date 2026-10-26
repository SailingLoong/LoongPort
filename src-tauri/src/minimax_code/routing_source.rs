//! Read-only routing facts from the original encrypted MiniMax owner.
//! No refresh, discovery, endpoint selection or second credential persistence.
use super::{catalog, Runtime};
use crate::{
    config_file_io,
    error::AppError,
    secrets::{owned_file::OwnedFile, session::SecretSession, storage::StorageContext},
    zcode_file_lock::FileLock,
};
use std::{sync::MutexGuard, time::Duration};

use crate::services::service_source_identity::{MiniMaxFact as Fact, MiniMaxStamp as Stamp};
/// Held through a route mutation's evidence check and commit. Never await while held.
pub(crate) struct ReadGuard<'a> {
    session: &'a SecretSession,
    session_epoch: u64,
    _runtime: MutexGuard<'static, Runtime>,
    _file: FileLock,
    facts: Vec<Fact>,
    unavailable: Vec<crate::services::service_source_identity::UnavailableNativeAccount>,
}
fn unavailable() -> AppError {
    AppError::Config("routing_groups.native_source_unavailable".into())
}
impl<'a> ReadGuard<'a> {
    /// Some original route writers already hold the supplied vault/SQLite lock.
    /// A nonblocking try-lock fails closed rather than waiting in reverse order.
    /// Do not reacquire session.read(): a pending vault writer could deadlock it.
    pub(crate) fn acquire_locked(
        session: &'a SecretSession,
        vault: &StorageContext,
        lock: &crate::services::native_source_guard::NativeReadLock,
    ) -> Result<Self, AppError> {
        session.ensure_available()?;
        let session_epoch = session.lifecycle_epoch();
        lock.check_context(session, vault)?;
        let path = session.root().join(catalog::FILE);
        let file = FileLock::acquire_recoverable(&path, Duration::from_millis(50))
            .map_err(|_| unavailable())?;
        let runtime = super::admit().map_err(|_| unavailable())?;
        let owner = OwnedFile::registered(catalog::FILE).map_err(|_| unavailable())?;
        let document: catalog::Document =
            match config_file_io::read_regular_file(&path, 4 * 1024 * 1024)
                .map_err(|_| unavailable())?
            {
                Some(bytes) => serde_json::from_slice(
                    &owner
                        .decode_storage(vault, &bytes)
                        .map_err(|_| unavailable())?,
                )
                .map_err(|_| unavailable())?,
                None => catalog::Document::default(),
            };
        catalog::validate(&document).map_err(|_| unavailable())?;
        use crate::services::service_source_identity::{
            LocalNativeUnavailable, UnavailableNativeAccount,
        };
        let unavailable = document
            .accounts
            .iter()
            .filter_map(|account| {
                let reason = match account.credentials.as_ref() {
                    None => LocalNativeUnavailable::AuthorizationAbsent,
                    Some(c)
                        if c.kind == catalog::CredentialKind::NativeAccessOnly
                            && c.expires_at <= catalog::now_ms()
                            && c.native_source
                                .as_ref()
                                .is_some_and(|source| !source.rejected) =>
                    {
                        LocalNativeUnavailable::CredentialsExpired
                    }
                    _ => return None,
                };
                Some(UnavailableNativeAccount {
                    account_id: account.id.clone(),
                    reason,
                })
            })
            .collect();
        let facts = document
            .accounts
            .iter()
            .filter(|account| {
                account.usable().is_ok()
                    && account
                        .credentials
                        .as_ref()
                        .is_some_and(|credentials| credentials.access_token.trim() != "sk-xxx")
            })
            .map(|account| Fact {
                stamp: Stamp {
                    account_id: account.id.clone(),
                    region: match account.region {
                        catalog::Region::Cn => "cn",
                        catalog::Region::Global => "global",
                    }
                    .into(),
                    auth_generation: account.auth_generation,
                    vault_id: vault.identity().store_id().to_owned(),
                    storage: vault.identity(),
                    real_user_id: account.real_user_id.clone(),
                    build_env: account.build_env.clone(),
                    session_epoch,
                    process_epoch: runtime.epoch,
                },
                label: account.label.clone(),
            })
            .collect();
        let guard = Self {
            session,
            session_epoch,
            _file: file,
            _runtime: runtime,
            facts,
            unavailable,
        };
        guard.check()?;
        Ok(guard)
    }
    pub(crate) fn check(&self) -> Result<(), AppError> {
        self.session.ensure_available()?;
        if self.session.lifecycle_epoch() != self.session_epoch {
            return Err(unavailable());
        }
        Ok(())
    }
    pub(crate) fn local_unavailability(
        &self,
        id: &str,
    ) -> Option<crate::services::service_source_identity::LocalNativeUnavailable> {
        self.check().ok()?;
        self.unavailable
            .iter()
            .find(|a| a.account_id == id)
            .map(|a| a.reason)
    }
    pub(crate) fn account(&self, id: &str) -> Option<&Fact> {
        self.check().ok()?;
        self.facts.iter().find(|fact| fact.stamp.account_id == id)
    }
}
