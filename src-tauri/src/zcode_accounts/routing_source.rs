//! Read-only routing facts from the original saved ZCode session owner.
//! This does not obtain credentials, refresh a session, or change consent.
use super::*;
use crate::{
    error::AppError,
    secrets::storage::StorageContext,
    services::{
        native_source_guard::NativeReadLock,
        service_source_identity::{
            LocalNativeUnavailable, NativeFact, NativeStamp, UnavailableNativeAccount, ZCodeStamp,
        },
    },
};

fn unavailable() -> AppError {
    AppError::Config("routing_groups.native_source_unavailable".into())
}

pub(crate) fn routing_facts(
    session: &SecretSession,
    vault: &StorageContext,
    lock: &NativeReadLock,
) -> Result<(Vec<NativeFact>, Vec<UnavailableNativeAccount>), AppError> {
    lock.check_context(session, vault)?;
    // Synthetic context selection requires the original sealed test capability.
    // A normal build retains the original supported-platform/default-library gate.
    #[cfg(all(any(test, feature = "test-hooks"), unix))]
    let context = fixture::routing_context(session, vault).map_err(|_| unavailable())?;
    #[cfg(not(all(any(test, feature = "test-hooks"), unix)))]
    let context = super::super::native_context::library_context(None).map_err(|_| unavailable())?;
    let result = facts_in_context(session, vault, &context)?;
    lock.check_context(session, vault)?;
    Ok(result)
}

// Caller retains the original sync-owner lock and vault read guard throughout.
pub(super) fn facts_in_context(
    session: &SecretSession,
    vault: &StorageContext,
    context: &LibraryContext,
) -> Result<(Vec<NativeFact>, Vec<UnavailableNativeAccount>), AppError> {
    let native = context.cipher().map_err(|_| unavailable())?;
    let store = VaultAccountStore::new(session.root(), vault).map_err(|_| unavailable())?;
    let status = store.catalog_status(&native).map_err(|_| unavailable())?;
    if status.pending || status.native_unconfirmed {
        return Err(unavailable());
    }
    let catalog = store
        .import_catalog(&native, &status.revision)
        .map_err(|_| unavailable())?;
    let mut facts = vec![];
    let mut unavailable_accounts = vec![];
    let mut seen = std::collections::HashSet::new();
    for snapshot in catalog.profiles() {
        let account_id = snapshot.identity().opaque_id();
        if !seen.insert(account_id.clone()) {
            return Err(unavailable());
        }
        let details = catalog.details(snapshot, &native);
        let rejected = details
            .evidence
            .as_ref()
            .is_some_and(SessionCheckReport::start_needs_login);
        // Catalog import authenticates every present encrypted value. Only the
        // original getter's explicit absence is skippable; malformed credential
        // format must not be converted into permission to select another owner.
        let token = match claim_jwt(&native, snapshot) {
            Ok(token) => Some(token),
            Err(super::super::official::OfficialError::Unauthorized) => None,
            Err(_) => return Err(unavailable()),
        };
        let reason = if rejected || token.is_none() {
            Some(LocalNativeUnavailable::AuthorizationAbsent)
        } else if token
            .as_ref()
            .is_some_and(|token| jwt_is_expired(token.expose(), now_seconds()))
        {
            Some(LocalNativeUnavailable::CredentialsExpired)
        } else {
            None
        };
        if let Some(reason) = reason {
            unavailable_accounts.push(UnavailableNativeAccount { account_id, reason });
            continue;
        }
        let binding = saved_session_binding(&native, snapshot).map_err(|_| unavailable())?;
        let storage = vault.identity();
        facts.push(NativeFact {
            stamp: NativeStamp::ZCode(ZCodeStamp {
                account_id,
                context_id: context.context_id().into(),
                vault_id: storage.store_id().to_owned(),
                storage,
                session_binding: hex::encode(binding),
                catalog_revision: status.revision.clone(),
                session_epoch: session.lifecycle_epoch(),
            }),
            label: details
                .label
                .filter(|label| !label.trim().is_empty())
                .unwrap_or_else(|| "ZCode".into()),
        });
    }
    Ok((facts, unavailable_accounts))
}
