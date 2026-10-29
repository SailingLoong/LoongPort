//! WorkBuddy CN resources and native transaction primitives; no native IPC activation.
pub(crate) mod authorization;
#[cfg(test)]
mod authorization_tests;
mod checkin;
pub(crate) mod credits;
pub(crate) mod engine;
#[cfg(test)]
mod engine_tests;
pub(crate) mod http;
pub(crate) mod model;
mod protocol;
mod store;
#[cfg(test)]
mod tests;

pub(crate) mod clock;

pub(crate) mod inference;
pub(crate) mod inference_commands;

#[cfg(any(test, debug_assertions, feature = "test-hooks"))]
pub(crate) mod integration_fixture;

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_plaintext_storage() {
    store::plaintext_tests::run();
}

#[cfg(feature = "test-hooks")]
pub(crate) mod shared_routing_fixture;

// Native owner primitives are compiled, not exposed as an activation command.
mod native;
pub(crate) mod native_owner;
mod native_transaction;

#[cfg(any(feature = "gui", test))]
pub(crate) mod native_preview;

#[cfg(test)]
mod native_account_tests;

mod native_source;
#[cfg(test)]
mod native_source_tests;
