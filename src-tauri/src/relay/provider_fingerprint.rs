//! Shared provider fingerprinting for cc-switch ownership reconciliation.
//!
//! A fingerprint is intentionally transient: it identifies two records that
//! point at the same upstream credential at the moment an import or provision
//! operation runs. It is not a database identity because keys can rotate.

use crate::app_config::AppType;
use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;

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

/// Remove non-managed providers that duplicate a newly written managed provider.
///
/// The comparison is scoped to one `AppType`: the same key may legitimately be
/// represented by distinct CLI configuration shapes, so matching another app's
/// provider would be an unsafe ownership inference.
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
    let current_id = db.get_current_provider(app_type.as_str())?;
    let providers = db.get_all_providers(app_type.as_str())?;
    let mut merged = Vec::new();

    for provider in providers.values() {
        if provider.id == managed_provider.id || crate::relay::is_managed(&provider.id) {
            continue;
        }
        if for_provider(provider, app_type).as_ref() != Some(&fingerprint) {
            continue;
        }

        let was_current = current_id.as_deref() == Some(provider.id.as_str());
        db.delete_provider(app_type.as_str(), &provider.id)?;
        merged.push(MergedProvider {
            name: provider.name.clone(),
            was_current,
        });
    }
    Ok(merged)
}
