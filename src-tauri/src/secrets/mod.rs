//! Credential protection primitives; persistence and lifecycle own their use.
pub(crate) mod bootstrap_restore;
mod crypto;
pub(crate) mod error;
pub(crate) mod files;
pub(crate) mod inventory;
pub(crate) mod key_store;
pub(crate) mod migration;
pub(crate) mod reset;
pub(crate) mod rewrap;
pub(crate) mod session;
#[cfg(feature = "gui")]
pub(crate) mod startup;
#[cfg(test)]
pub(crate) mod testing;
pub(crate) mod transition;
#[cfg(test)]
mod upgrade_diag_test;

pub(crate) use crypto::{VaultContext, VaultMetadata};
pub(crate) use error::SecretError;
