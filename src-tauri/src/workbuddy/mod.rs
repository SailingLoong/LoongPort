//! WorkBuddy CN account resources; no native session access.
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
