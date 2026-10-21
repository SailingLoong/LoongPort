//! Ephemeral validity of an OpenCode account-owned API-key response.
//!
//! The original vendor resolver still validates the official destination and
//! resolves the key. Acquire before that resolver; check again after resolution,
//! before send, and through the existing final-response lease. This module never
//! decrypts a vendor key, sends HTTP, or interprets output bytes.
use crate::{
    app_config::AppType,
    database::Database,
    provider::{AuthBindingSource, Provider, ProviderMeta},
    proxy::ProxyError,
};
use rusqlite::OptionalExtension;
use std::sync::Arc;

fn unavailable() -> ProxyError {
    // Authorization changes are terminal. They must not become an AuthError
    // that the technical-failure policy could replay with another account.
    ProxyError::Internal("vendor_response.authorization_changed".into())
}

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    row_id: i64,
    account: String,
}

fn identity(meta: &ProviderMeta) -> Result<Identity, ProxyError> {
    let binding = meta.auth_binding.as_ref().ok_or_else(unavailable)?;
    if binding.source != AuthBindingSource::VendorApiKey
        || binding.auth_provider.as_deref() != Some("opencode")
    {
        return Err(unavailable());
    }
    let raw_id = binding.account_id.as_deref().ok_or_else(unavailable)?;
    let row_id = raw_id.parse::<i64>().map_err(|_| unavailable())?;
    // Match the original owner's exact decimal row identity; no default account.
    if row_id <= 0 || row_id.to_string() != raw_id {
        return Err(unavailable());
    }
    let account = meta
        .loongport_vendor_account
        .as_ref()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(unavailable)?
        .clone();
    Ok(Identity { row_id, account })
}

struct Snapshot {
    epoch: u64,
    encrypted_key_digest: String,
}

struct Inner {
    db: Arc<Database>,
    app: AppType,
    provider_id: String,
    identity: Identity,
    snapshot: Snapshot,
}

/// Opaque original-owner references only. Neither Debug nor accessors expose
/// the private fingerprint; clones share the same request-local snapshot.
#[derive(Clone)]
pub(crate) struct Fence(Arc<Inner>);

impl Fence {
    pub(crate) fn acquire(
        db: Arc<Database>,
        app: AppType,
        provider: &Provider,
    ) -> Result<Option<Self>, ProxyError> {
        let Some(meta) = provider.meta.as_ref().filter(|meta| {
            meta.auth_binding.as_ref().is_some_and(|binding| {
                binding.source == AuthBindingSource::VendorApiKey
                    && binding.auth_provider.as_deref() == Some("opencode")
            })
        }) else {
            return Ok(None);
        };
        if !matches!(app, AppType::Claude | AppType::Codex) || provider.id.trim().is_empty() {
            return Err(unavailable());
        }
        let identity = identity(meta)?;
        let snapshot = snapshot(&db, &app, &provider.id, &identity)?;
        Ok(Some(Self(Arc::new(Inner {
            db,
            app,
            provider_id: provider.id.clone(),
            identity,
            snapshot,
        }))))
    }

    /// Short synchronous original-owner read; no lock is retained across await.
    pub(crate) fn check(&self) -> Result<(), ProxyError> {
        let inner = &self.0;
        let current = snapshot(&inner.db, &inner.app, &inner.provider_id, &inner.identity)?;
        if current.epoch != inner.snapshot.epoch
            || current.encrypted_key_digest != inner.snapshot.encrypted_key_digest
        {
            return Err(unavailable());
        }
        Ok(())
    }
}

fn snapshot(
    db: &Database,
    app: &AppType,
    provider_id: &str,
    expected: &Identity,
) -> Result<Snapshot, ProxyError> {
    // Preserve the original vendor owner's vault -> connection lock order.
    // The DB carries its own exact SecretSession; no caller-selected root/session
    // can authorize a read from a different library.
    let vault = db.secrets.read().map_err(|_| unavailable())?;
    let epoch = db.secrets.lifecycle_epoch();
    let conn = db.conn.lock().map_err(|_| unavailable())?;
    let meta = Database::get_provider_meta_on_connection(&conn, &vault, provider_id, app.as_str())
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    if identity(&meta)? != *expected {
        return Err(unavailable());
    }
    let row: Option<(String, Option<String>, String)> = conn
        .query_row(
            "SELECT vendor_id,account_id,api_key FROM loongport_vendor WHERE id=?1",
            [expected.row_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| unavailable())?;
    let (vendor, account, encrypted_key) = row.ok_or_else(unavailable)?;
    if vendor != "opencode"
        || account.as_deref() != Some(expected.account.as_str())
        || encrypted_key.is_empty()
    {
        return Err(unavailable());
    }
    // set_api_key reseals this existing owner column with a fresh random nonce.
    // Thus even reissuing identical plaintext retires the earlier snapshot.
    // Token-only refresh, display edits and ordering do not change this column.
    let encrypted_key_digest = crate::live::engine::sha256_hex(encrypted_key.as_bytes());
    db.secrets.ensure_available().map_err(|_| unavailable())?;
    if db.secrets.lifecycle_epoch() != epoch {
        return Err(unavailable());
    }
    Ok(Snapshot {
        epoch,
        encrypted_key_digest,
    })
}
