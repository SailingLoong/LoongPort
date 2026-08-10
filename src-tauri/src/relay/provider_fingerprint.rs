//! Shared provider fingerprinting for cc-switch ownership reconciliation.
//!
//! A fingerprint is intentionally transient: it identifies two records that
//! point at the same upstream credential at the moment an import or provision
//! operation runs. It is not a database identity because keys can rotate.

use crate::app_config::AppType;
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

/// Build the same transient fingerprint from provision's known credentials.
pub(crate) fn for_credentials(site_origin: &str, api_key: &str) -> Option<(String, String)> {
    let origin = crate::relay::api::normalize_site_origin(site_origin).ok()?;
    if origin.is_empty() || api_key.is_empty() {
        return None;
    }
    Some((origin, api_key.to_string()))
}
