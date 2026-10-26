//! One existing sync-owner lock for all original native facts in a shared snapshot.
use super::service_source_identity::{
    LocalNativeUnavailable, NativeFact, NativeStamp, UnavailableNativeAccount,
};
use crate::{
    error::AppError,
    secrets::{
        session::SecretSession,
        storage::{StorageContext, StorageIdentity},
    },
};
fn unavailable() -> AppError {
    AppError::Config("routing_groups.native_source_unavailable".into())
}
pub(crate) struct NativeReadLock<'a> {
    session: &'a SecretSession,
    epoch: u64,
    binding: StorageIdentity,
    _sync: tokio::sync::MutexGuard<'static, ()>,
}
impl NativeReadLock<'_> {
    pub(crate) fn check_context(
        &self,
        session: &SecretSession,
        vault: &StorageContext,
    ) -> Result<(), AppError> {
        if !std::ptr::eq(self.session, session) || vault.identity() != self.binding {
            return Err(unavailable());
        }
        self.check()
    }
    fn check(&self) -> Result<(), AppError> {
        self.session.ensure_available()?;
        if self.session.lifecycle_epoch() != self.epoch {
            return Err(unavailable());
        }
        Ok(())
    }
}
pub(crate) struct NativeReadGuard<'a> {
    // Fields drop in declaration order: platform locks first, shared sync last.
    minimax: Option<crate::minimax_code::routing_source::ReadGuard<'a>>,
    workbuddy: Vec<NativeFact>,
    workbuddy_unavailable: Vec<UnavailableNativeAccount>,
    zcode: Vec<NativeFact>,
    zcode_unavailable: Vec<UnavailableNativeAccount>,
    lock: NativeReadLock<'a>,
}
impl<'a> NativeReadGuard<'a> {
    pub(crate) fn acquire(
        session: &'a SecretSession,
        vault: &StorageContext,
        minimax: bool,
        workbuddy: bool,
        zcode: bool,
    ) -> Result<Self, AppError> {
        session.ensure_available()?;
        let lock = NativeReadLock {
            session,
            epoch: session.lifecycle_epoch(),
            binding: vault.identity(),
            _sync: crate::services::sync_protocol::sync_mutex()
                .try_lock()
                .map_err(|_| unavailable())?,
        };
        let minimax = if minimax {
            Some(
                crate::minimax_code::routing_source::ReadGuard::acquire_locked(
                    session, vault, &lock,
                )?,
            )
        } else {
            None
        };
        let (workbuddy, workbuddy_unavailable) = if workbuddy {
            crate::workbuddy::inference::routing_facts(session, vault, &lock)?
        } else {
            (vec![], vec![])
        };
        let (zcode, zcode_unavailable) = if zcode {
            #[cfg(any(not(test), unix))]
            {
                crate::zcode_accounts::inference::routing_facts(session, vault, &lock)?
            }
            #[cfg(all(test, not(unix)))]
            {
                return Err(unavailable());
            }
        } else {
            (vec![], vec![])
        };
        lock.check_context(session, vault)?;
        Ok(Self {
            minimax,
            workbuddy,
            workbuddy_unavailable,
            zcode,
            zcode_unavailable,
            lock,
        })
    }
    pub(crate) fn check(&self) -> Result<(), AppError> {
        self.lock.check()?;
        if let Some(mm) = &self.minimax {
            mm.check()?;
        }
        Ok(())
    }
    pub(crate) fn local_unavailability(
        &self,
        platform: &str,
        id: &str,
    ) -> Option<LocalNativeUnavailable> {
        self.check().ok()?;
        match platform {
            "minimax_code" => self.minimax.as_ref()?.local_unavailability(id),
            "zcode_start_plan" => self
                .zcode_unavailable
                .iter()
                .find(|a| a.account_id == id)
                .map(|a| a.reason),
            "workbuddy" => self
                .workbuddy_unavailable
                .iter()
                .find(|a| a.account_id == id)
                .map(|a| a.reason),
            _ => None,
        }
    }
    pub(crate) fn account(&self, platform: &str, id: &str) -> Option<NativeFact> {
        self.check().ok()?;
        match platform {
            "minimax_code" => self.minimax.as_ref()?.account(id).map(|fact| NativeFact {
                stamp: NativeStamp::MiniMax(fact.stamp.clone()),
                label: fact.label.clone(),
            }),
            "zcode_start_plan" => {
                let mut found = self.zcode.iter().filter(|fact| matches!(&fact.stamp, NativeStamp::ZCode(stamp) if stamp.account_id == id));
                let row = found.next()?;
                if found.next().is_some() {
                    None
                } else {
                    Some(row.clone())
                }
            }
            "workbuddy" => {
                let mut found=self.workbuddy.iter().filter(|fact|matches!(&fact.stamp,NativeStamp::WorkBuddy(stamp) if stamp.account_id==id));
                let row = found.next()?;
                if found.next().is_some() {
                    None
                } else {
                    Some(row.clone())
                }
            }
            _ => None,
        }
    }
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
#[path = "shared_pool_storage_fixture.rs"]
pub(crate) mod storage_fixture;
