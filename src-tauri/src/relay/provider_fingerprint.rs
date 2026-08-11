//! Shared provider fingerprinting for cc-switch ownership reconciliation.
//!
//! A fingerprint is intentionally transient: it identifies two records that
//! point at the same upstream credential at the moment an import or provision
//! operation runs. It is not a database identity because keys can rotate.

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use rusqlite::{params, TransactionBehavior};

/// Return the normalized `(site origin, api key)` fingerprint for a provider.
pub(crate) fn for_provider(provider: &Provider, app_type: &AppType) -> Option<(String, String)> {
    let base_url = crate::proxy::providers::get_adapter(app_type)
        .extract_base_url(provider)
        .ok()?;
    let origin = crate::relay::api::normalize_site_origin(&base_url).ok()?;
    let api_key = crate::relay::provision::extract_api_key(&provider.settings_config, app_type)?;
    if origin.is_empty() || api_key.is_empty() {
        return None;
    }
    Some((origin, api_key))
}

#[derive(Debug)]
pub(crate) struct MergedProvider {
    pub name: String,
    pub was_current: bool,
}

/// Atomically adopt non-managed providers that duplicate a newly written managed provider.
///
/// The comparison is scoped to one `AppType`: the same key may legitimately be
/// represented by distinct CLI configuration shapes, so matching another app's
/// provider would be an unsafe ownership inference.
///
/// If a duplicate is current, deleting it and transferring current ownership to
/// the managed provider happen in the same transaction. A failed transfer rolls
/// back every duplicate deletion.
pub(crate) fn remove_unmanaged_duplicates(
    db: &Database,
    app_type: &AppType,
    managed_provider: &Provider,
) -> Result<Vec<MergedProvider>, AppError> {
    if !crate::relay::is_managed(&managed_provider.id) {
        return Ok(Vec::new());
    }
    let Some(fingerprint) = for_provider(managed_provider, app_type) else {
        return Ok(Vec::new());
    };
    let mut conn = crate::database::lock_conn!(db.conn);
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| AppError::Database(error.to_string()))?;
    let (current_id, providers) =
        Database::get_provider_snapshot_in_transaction(&tx, app_type.as_str())?;
    let mut duplicates = Vec::new();

    for provider in providers.values() {
        if provider.id == managed_provider.id || crate::relay::is_managed(&provider.id) {
            continue;
        }
        if for_provider(provider, app_type).as_ref() != Some(&fingerprint) {
            continue;
        }

        let was_current = current_id.as_deref() == Some(provider.id.as_str());
        duplicates.push((
            provider.id.clone(),
            MergedProvider {
                name: provider.name.clone(),
                was_current,
            },
        ));
    }
    if duplicates.is_empty() {
        return Ok(Vec::new());
    }

    let transfer_current = duplicates.iter().any(|(_, merged)| merged.was_current);
    for (provider_id, _) in &duplicates {
        tx.execute(
            "DELETE FROM providers WHERE id = ?1 AND app_type = ?2",
            params![provider_id, app_type.as_str()],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
    }
    if transfer_current {
        tx.execute(
            "UPDATE providers SET is_current = 0 WHERE app_type = ?1",
            params![app_type.as_str()],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        let updated = tx
            .execute(
                "UPDATE providers SET is_current = 1 WHERE id = ?1 AND app_type = ?2",
                params![managed_provider.id, app_type.as_str()],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if updated != 1 {
            return Err(AppError::Database(format!(
                "托管 provider {} 不存在，无法转移当前项",
                managed_provider.id
            )));
        }
    }
    tx.commit()
        .map_err(|error| AppError::Database(error.to_string()))?;

    Ok(duplicates.into_iter().map(|(_, merged)| merged).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn duplicate_adoption_snapshots_inside_an_immediate_transaction() {
        let source = include_str!("provider_fingerprint.rs");
        let start = source
            .find("pub(crate) fn remove_unmanaged_duplicates")
            .expect("duplicate adoption function exists");
        let end = source[start..]
            .find("\n#[cfg(test)]")
            .expect("duplicate adoption test module follows the function");
        let function = &source[start..start + end];
        let immediate_transaction = function
            .find("transaction_with_behavior(TransactionBehavior::Immediate)")
            .expect("duplicate adoption must begin an IMMEDIATE transaction");
        let snapshot = function
            .find("Database::get_provider_snapshot_in_transaction")
            .expect("duplicate adoption must read its provider snapshot through the transaction");
        let matching = function
            .find("for provider in providers.values()")
            .expect("duplicate adoption still matches providers");
        let deletion = function
            .find("DELETE FROM providers")
            .expect("duplicate adoption still deletes duplicates");
        let current_transfer = function
            .find("UPDATE providers SET is_current = 0")
            .expect("duplicate adoption still transfers current ownership");
        let committed = function
            .find("tx.commit()")
            .expect("duplicate adoption still commits its transaction");

        assert!(
            immediate_transaction < snapshot
                && snapshot < matching
                && matching < deletion
                && deletion < current_transfer
                && current_transfer < committed,
            "the provider snapshot, matching, deletion, and current transfer must stay in one immediate transaction"
        );
    }
}
