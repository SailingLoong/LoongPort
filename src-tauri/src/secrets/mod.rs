//! Credential protection primitives; persistence and lifecycle own their use.
pub(crate) mod bootstrap_restore;
mod crypto;
pub(crate) mod error;
pub(crate) mod export;
pub(crate) mod files;
pub(crate) mod inventory;
pub(crate) mod key_store;
pub(crate) mod migration;
pub(crate) mod owned_file;
pub(crate) mod reset;
pub(crate) mod rewrap;
pub(crate) mod session;
#[cfg(any(test, feature = "gui", feature = "test-hooks"))]
pub(crate) mod startup;
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) mod testing;
pub(crate) mod transition;
pub(crate) mod upgrade;
pub(crate) use crypto::{VaultContext, VaultMetadata};
pub(crate) use error::SecretError;
