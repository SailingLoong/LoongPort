//! MiniMax Code credentials and check-in operations share one encrypted owner.
pub(crate) mod auth;
pub(crate) mod catalog;
pub(crate) mod checkin;
pub(crate) mod entitlement;
pub(crate) mod http;
pub(crate) mod inference;
mod native_format;
pub(crate) mod native_import;

#[cfg(test)]
mod tests;

use std::sync::{Mutex, MutexGuard, OnceLock};
#[derive(Default)]
pub(super) struct Runtime {
    epoch: u64,
    stopped: bool,
}
fn runtime() -> &'static Mutex<Runtime> {
    static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();
    RUNTIME.get_or_init(Default::default)
}
pub(super) fn admit() -> Result<MutexGuard<'static, Runtime>, catalog::Failure> {
    admit_from(runtime())
}
fn admit_from(runtime: &Mutex<Runtime>) -> Result<MutexGuard<'_, Runtime>, catalog::Failure> {
    let guard = runtime
        .lock()
        .map_err(|_| catalog::Failure::StorageUnavailable)?;
    if guard.stopped {
        return Err(catalog::Failure::StaleIdentity);
    }
    Ok(guard)
}
pub(crate) fn shutdown() {
    // No network or async owner is waited on. A local durable commit that has
    // already won admission finishes before shutdown returns.
    if let Ok(mut guard) = runtime().lock() {
        guard.stopped = true;
        guard.epoch = guard.epoch.saturating_add(1);
        auth::clear_flows();
        native_import::clear_flows();
    }
    // A poisoned Runtime remains poisoned: all future admission fails closed.
}
/// The one process-wide MiniMax test lock. Hold until all fixture IO/body work
/// finishes. Only this cfg(test) entry can combine serialization with reset.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) async fn isolated_fixture_guard() -> tokio::sync::OwnedMutexGuard<()> {
    static SERIAL: OnceLock<std::sync::Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    let serial = SERIAL
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
        .lock_owned()
        .await;
    reset_for_isolated_fixture();
    serial
}

#[cfg(any(test, feature = "test-hooks"))]
fn reset_for_isolated_fixture() {
    let mut guard = runtime().lock().unwrap();
    guard.epoch += 1;
    guard.stopped = false;
    auth::clear_flows();
    native_import::clear_flows();
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    #[test]
    fn poisoned_runtime_cannot_reopen_admission() {
        let runtime = std::sync::Arc::new(Mutex::new(Runtime::default()));
        let copy = runtime.clone();
        assert!(std::thread::spawn(move || {
            let _guard = copy.lock().unwrap();
            panic!("synthetic poison");
        })
        .join()
        .is_err());
        assert!(matches!(
            admit_from(&runtime),
            Err(catalog::Failure::StorageUnavailable)
        ));
        assert!(matches!(
            admit_from(&runtime),
            Err(catalog::Failure::StorageUnavailable)
        ));
    }
}

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub(crate) mod refresh_owner_fixture;

#[cfg(any(test, feature = "test-hooks"))]
mod test_fixtures;
