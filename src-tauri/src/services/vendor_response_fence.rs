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

#[derive(Clone, PartialEq, Eq)]
struct Snapshot {
    storage: crate::secrets::storage::StorageIdentity,
    epoch: u64,
    encrypted_key_digest: String,
    has_key: bool,
}

/// Selection intent only, never a credential lease. An empty later key can be
/// captured without preventing an earlier usable member from being attempted.
#[derive(Clone)]
pub(crate) struct Expected {
    app: AppType,
    provider_id: String,
    identity: Identity,
    snapshot: Snapshot,
}

pub(crate) fn owns(provider: &Provider) -> bool {
    provider.meta.as_ref().is_some_and(|meta| {
        meta.auth_binding.as_ref().is_some_and(|binding| {
            binding.source == AuthBindingSource::VendorApiKey
                && binding.auth_provider.as_deref() == Some("opencode")
        })
    })
}

/// Only an explicit empty original key column is locally unavailable. Missing
/// rows, invalid destinations and corrupt nonempty ciphertext are not empty.
pub(crate) enum LocalKeyAvailability {
    Present,
    Missing,
}

pub(crate) fn local_availability_on(
    conn: &rusqlite::Connection,
    app: &AppType,
    provider: &Provider,
) -> Result<Option<LocalKeyAvailability>, ProxyError> {
    if !owns(provider) {
        return Ok(None);
    }
    super::vendor_source_binding::validate_reference_on(conn, app, provider)
        .map_err(|_| unavailable())?;
    let expected = identity(provider.meta.as_ref().ok_or_else(unavailable)?)?;
    let (_, has_key) = key_state_on(conn, &expected)?;
    Ok(Some(if has_key {
        LocalKeyAvailability::Present
    } else {
        LocalKeyAvailability::Missing
    }))
}

impl Expected {
    pub(crate) fn capture_on(
        conn: &rusqlite::Connection,
        vault: &crate::secrets::storage::StorageContext,
        epoch: u64,
        app: &AppType,
        provider: &Provider,
    ) -> Result<Option<Self>, ProxyError> {
        if !owns(provider) {
            return Ok(None);
        }
        if !matches!(app, AppType::Claude | AppType::Codex) || provider.id.trim().is_empty() {
            return Err(unavailable());
        }
        let identity = identity(provider.meta.as_ref().ok_or_else(unavailable)?)?;
        let snapshot = snapshot_on(conn, vault, epoch, app, &provider.id, &identity)?;
        Ok(Some(Self {
            app: app.clone(),
            provider_id: provider.id.clone(),
            identity,
            snapshot,
        }))
    }
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
        Self::acquire_inner(db, app, provider, None)
    }

    pub(crate) fn acquire_with_expected(
        db: Arc<Database>,
        app: AppType,
        provider: &Provider,
        expected: &Expected,
    ) -> Result<Option<Self>, ProxyError> {
        Self::acquire_inner(db, app, provider, Some(expected))
    }

    fn acquire_inner(
        db: Arc<Database>,
        app: AppType,
        provider: &Provider,
        expected: Option<&Expected>,
    ) -> Result<Option<Self>, ProxyError> {
        if !owns(provider) {
            return if expected.is_some() {
                Err(unavailable())
            } else {
                Ok(None)
            };
        }
        let meta = provider.meta.as_ref().ok_or_else(unavailable)?;
        if !matches!(app, AppType::Claude | AppType::Codex) || provider.id.trim().is_empty() {
            return Err(unavailable());
        }
        let identity = identity(meta)?;
        let snapshot = snapshot(&db, &app, &provider.id, &identity)?;
        if expected.is_some_and(|expected| {
            expected.app != app
                || expected.provider_id != provider.id
                || expected.identity != identity
                || expected.snapshot != snapshot
        }) {
            return Err(unavailable());
        }

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
        if current != inner.snapshot {
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
    let snapshot = snapshot_on(&conn, &vault, epoch, app, provider_id, expected)?;
    db.secrets.ensure_available().map_err(|_| unavailable())?;
    if db.secrets.lifecycle_epoch() != epoch || !snapshot.has_key {
        return Err(unavailable());
    }
    Ok(snapshot)
}

fn snapshot_on(
    conn: &rusqlite::Connection,
    vault: &crate::secrets::storage::StorageContext,
    epoch: u64,
    app: &AppType,
    provider_id: &str,
    expected: &Identity,
) -> Result<Snapshot, ProxyError> {
    let meta = Database::get_provider_meta_on_connection(conn, vault, provider_id, app.as_str())
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    if identity(&meta)? != *expected {
        return Err(unavailable());
    }
    let (encrypted_key_digest, has_key) = key_state_on(conn, expected)?;
    Ok(Snapshot {
        storage: vault.identity(),
        epoch,
        encrypted_key_digest,
        has_key,
    })
}

/// Review evidence from the same original owner while the caller holds its
/// vault/SQLite publication transaction. No plaintext key or extra lock is read.
pub(crate) fn review_evidence_on(
    conn: &rusqlite::Connection,
    provider: &Provider,
) -> Result<Option<String>, ProxyError> {
    let Some(meta) = provider.meta.as_ref().filter(|meta| {
        meta.auth_binding.as_ref().is_some_and(|binding| {
            binding.source == AuthBindingSource::VendorApiKey
                && binding.auth_provider.as_deref() == Some("opencode")
        })
    }) else {
        return Ok(None);
    };
    let (digest, has_key) = key_state_on(conn, &identity(meta)?)?;
    if !has_key {
        return Err(unavailable());
    }
    Ok(Some(digest))
}

fn key_state_on(
    conn: &rusqlite::Connection,
    expected: &Identity,
) -> Result<(String, bool), ProxyError> {
    let row: Option<(String, Option<String>, String, String)> = conn
        .query_row(
            "SELECT vendor_id,account_id,api_key,api_key_revision FROM loongport_vendor WHERE id=?1",
            [expected.row_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|_| unavailable())?;
    let (vendor, account, encrypted_key, revision) = row.ok_or_else(unavailable)?;
    if vendor != "opencode"
        || account.as_deref() != Some(expected.account.as_str())
        || !crate::database::credential_revision::valid(&revision)
    {
        return Err(unavailable());
    }
    // Original key writers rotate the private original-row incarnation in the
    // same SQL write, including plaintext same-key reissue. Display, ordering and
    // web-login token updates never rotate it. No revision is generated here.
    Ok((
        crate::live::engine::sha256_hex(format!("{revision}\0{encrypted_key}").as_bytes()),
        !encrypted_key.is_empty(),
    ))
}
