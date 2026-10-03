//! Compile actual product codecs, never a parallel vault implementation.
//! This crate has no runtime entry point; even its source graph is test-only.
#![cfg(test)]

#[path = "../../src-tauri/src/error.rs"]
pub mod error;

#[path = "../../src-tauri/src/config_file_io.rs"]
mod config_file_io;

#[path = "../../src-tauri/src/zcode_file_lock.rs"]
mod zcode_file_lock;

// Existing product lifecycle helpers have callers outside this narrow graph.
#[allow(dead_code)]
#[path = "../../src-tauri/src/secrets/error.rs"]
pub(crate) mod secret_error;
use secret_error::SecretError;

#[allow(dead_code)]
#[path = "../../src-tauri/src/secrets/crypto.rs"]
mod vault_crypto;

#[path = "../../src-tauri/src/secrets/owned_file.rs"]
pub(crate) mod owned_file;

mod secrets {
    pub(crate) use crate::owned_file;
    pub(crate) use crate::secret_error as error;
    pub(crate) use crate::vault_crypto::{VaultContext, VaultMetadata};
}

#[path = "../../src-tauri/src/zcode_accounts/mod.rs"]
mod zcode_accounts;
