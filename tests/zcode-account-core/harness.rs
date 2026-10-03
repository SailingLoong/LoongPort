//! Compile actual product codecs, never a parallel vault implementation.
//! This crate has no runtime entry point; even its source graph is test-only.
#![cfg(test)]

#[path = "../../src-tauri/src/error.rs"]
pub mod error;

// Existing product lifecycle helpers have callers outside this narrow graph.
#[allow(dead_code)]
#[path = "../../src-tauri/src/secrets/error.rs"]
mod secret_error;
use secret_error::SecretError;

#[allow(dead_code)]
#[path = "../../src-tauri/src/secrets/crypto.rs"]
mod vault_crypto;

mod secrets {
    pub(crate) use crate::vault_crypto::VaultContext;
}

#[path = "../../src-tauri/src/zcode_accounts/mod.rs"]
mod zcode_accounts;
