//! Native read-only identity/model seam shared by source presentation and routing.
//! Callers retain ownership of the provider snapshot and operational eligibility.
use crate::{
    app_config::AppType,
    error::AppError,
    provider::Provider,
    relay::{identity::same_site_identity, managed, provider_config},
    services::{
        service_sources::{verified_relay_account, SourceAccountReference},
        vendor_source_binding,
    },
};
use rusqlite::Connection;

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct MiniMaxStamp {
    pub account_id: String,
    pub region: String,
    pub auth_generation: u64,
    pub vault_id: String,
    #[serde(skip)]
    pub storage: crate::secrets::storage::StorageIdentity,
    pub real_user_id: String,
    pub build_env: String,
    #[serde(skip)]
    pub session_epoch: u64,
    #[serde(skip)]
    pub process_epoch: u64,
}
#[derive(Debug, Clone)]
pub(crate) struct MiniMaxFact {
    pub stamp: MiniMaxStamp,
    pub label: String,
}
#[derive(Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct WorkBuddyStamp {
    pub account_id: String,
    pub vault_id: String,
    #[serde(skip)]
    pub storage: crate::secrets::storage::StorageIdentity,
    pub uid: String,
    pub domain: String,
    pub legacy_fingerprint: String,
    pub original_grant_id: String,
    pub inference_revision: u64,
    pub generation: u64,
    pub credential_binding: String,
    #[serde(skip)]
    pub session_epoch: u64,
}
/// Original ZCode account/session proof; no JWT or native path leaves its owner.
// Match the original inference owner: non-Unix unit builds cannot mint this proof.
#[cfg(any(not(test), unix))]
#[derive(Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct ZCodeStamp {
    pub account_id: String,
    pub context_id: String,
    pub vault_id: String,
    #[serde(skip)]
    pub storage: crate::secrets::storage::StorageIdentity,
    pub session_binding: String,
    #[serde(skip)]
    pub catalog_revision: String,
    #[serde(skip)]
    pub session_epoch: u64,
}
#[derive(Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct QoderStamp {
    pub fact: crate::database::QoderRoutingSourceFact,
    #[serde(skip)]
    pub session_epoch: u64,
}
#[derive(Clone, serde::Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum NativeStamp {
    MiniMax(MiniMaxStamp),
    WorkBuddy(WorkBuddyStamp),
    Qoder(QoderStamp),
    #[cfg(any(not(test), unix))]
    ZCode(ZCodeStamp),
}
impl std::fmt::Debug for NativeStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MiniMax(_) => "MiniMax(owner proof)",
            Self::WorkBuddy(_) => "WorkBuddy(owner proof)",
            Self::Qoder(_) => "Qoder(owner proof)",
            #[cfg(any(not(test), unix))]
            Self::ZCode(_) => "ZCode(owner proof)",
        })
    }
}
/// Only original local-owner facts, never an upstream error classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalNativeUnavailable {
    PolicyPaused,
    UserRevoked,
    AuthorizationAbsent,
    CredentialsExpired,
    AuthorizationPending,
}
#[derive(Clone)]
pub(crate) struct UnavailableNativeAccount {
    pub account_id: String,
    pub reason: LocalNativeUnavailable,
}
#[derive(Clone)]
pub(crate) struct NativeFact {
    pub stamp: NativeStamp,
    pub label: String,
}
#[cfg(any(feature = "gui", test))]
pub(crate) use super::native_source_guard::NativeReadGuard;
#[cfg(not(any(feature = "gui", test)))]
pub(crate) struct NativeReadGuard<'a>(std::marker::PhantomData<&'a ()>);
#[cfg(not(any(feature = "gui", test)))]
impl<'a> NativeReadGuard<'a> {
    pub(crate) fn acquire(
        _: &'a crate::secrets::session::SecretSession,
        _: &crate::secrets::storage::StorageContext,
        _: bool,
        _: bool,
        _: bool,
    ) -> Result<Self, AppError> {
        Err(AppError::Config(
            "routing_groups.native_source_unavailable".into(),
        ))
    }
    pub(crate) fn check(&self) -> Result<(), AppError> {
        Err(AppError::Config(
            "routing_groups.native_source_unavailable".into(),
        ))
    }
    pub(crate) fn account(&self, _: &str, _: &str) -> Option<NativeFact> {
        None
    }
    pub(crate) fn local_unavailability(&self, _: &str, _: &str) -> Option<LocalNativeUnavailable> {
        None
    }
}
pub(crate) use super::service_sources::native_source_id;

#[derive(Debug)]
pub(crate) struct NativeSourceBinding {
    pub source_id: String,
    pub account: SourceAccountReference,
    pub app_id: String,
    pub provider_id: String,
    pub model: Option<String>,
    pub native_stamp: Option<NativeStamp>,
    pub local_provider: Option<super::openrouter_source::SourceFact>,
}

/// Identity queries deliberately exclude credentials. No discovery, refresh, writes,
/// or application state construction occurs. Missing/ambiguous ownership fails closed.
pub(crate) fn binding_on(
    conn: &Connection,
    app: &AppType,
    provider: &Provider,
) -> Result<Option<NativeSourceBinding>, AppError> {
    if !managed::is_managed(&provider.id) {
        return Ok(None);
    }
    let account = if managed::is_managed_vendor(&provider.id) {
        if !crate::vendor::provision::VENDOR_APPS.contains(app) {
            return Ok(None);
        }
        match vendor_source_binding::identity(conn, &provider.id) {
            Ok((id, _, _, _)) => SourceAccountReference::Vendor { id },
            Err(AppError::Config(reason)) if reason == "source.invalid_vendor_binding" => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
    } else {
        let recorded = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.loongport_account_id);
        let accounts = crate::services::service_sources::relay_account_identities(conn)?;
        let mut statement = conn.prepare("SELECT id,site_origin FROM loongport_relay")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut owner = None;
        for row in rows {
            let (id, site) = row?;
            let expected = accounts.get(&id).copied().flatten();
            if same_site_identity(provider.website_url.as_deref(), Some(&site))
                && verified_relay_account(expected, recorded)
            {
                if owner.is_some() {
                    return Ok(None);
                }
                owner = Some(id);
            }
        }
        let Some(id) = owner else {
            return Ok(None);
        };
        SourceAccountReference::Relay { id }
    };
    Ok(Some(NativeSourceBinding {
        source_id: provider.id.clone(),
        account,
        app_id: app.as_str().into(),
        provider_id: provider.id.clone(),
        local_provider: None,
        native_stamp: None,
        model: provider_config::selected_model(app, &provider.settings_config)
            .filter(|model| !model.trim().is_empty()),
    }))
}

/// Validated original account projection only; custom endpoint/key rows are rejected.
pub(crate) fn native_platform(provider: &Provider) -> Option<&'static str> {
    match crate::proxy::managed_accounts::selected(provider) {
        Some((crate::proxy::managed_accounts::AccountKind::MiniMax, _)) => Some("minimax_code"),
        Some((crate::proxy::managed_accounts::AccountKind::WorkBuddy, _)) => Some("workbuddy"),
        Some((crate::proxy::managed_accounts::AccountKind::ZCode, _)) => Some("zcode_start_plan"),
        _ => None,
    }
}
/// One model-intent check for preview/CAS and the existing router. Native
/// account projections retain exact configured-model semantics.
pub(crate) fn member_model_supported_on(
    conn: &Connection,
    app: &AppType,
    provider: &Provider,
    source: &NativeSourceBinding,
    model: &str,
) -> Result<bool, AppError> {
    if source.model.as_deref() == Some(model) {
        return Ok(true);
    }
    if !matches!(source.account, SourceAccountReference::Vendor { .. }) {
        return Ok(false);
    }
    vendor_source_binding::shared_model_supported_on(conn, app, provider, model)
}

pub(crate) fn read_native_for_providers<'a>(
    session: &'a crate::secrets::session::SecretSession,
    vault: &crate::secrets::storage::StorageContext,
    providers: &indexmap::IndexMap<String, Provider>,
) -> Result<Option<NativeReadGuard<'a>>, AppError> {
    let minimax = providers
        .values()
        .any(|p| native_platform(p) == Some("minimax_code"));
    let workbuddy = providers
        .values()
        .any(|p| native_platform(p) == Some("workbuddy"));
    let zcode = providers
        .values()
        .any(|p| native_platform(p) == Some("zcode_start_plan"));
    if minimax || workbuddy || zcode {
        NativeReadGuard::acquire(session, vault, minimax, workbuddy, zcode).map(Some)
    } else {
        Ok(None)
    }
}
fn configured_native_projection(app: &AppType, provider: &Provider) -> Option<(String, String)> {
    native_platform(provider)?;
    crate::services::provider::managed_inference::validate(app, provider).ok()?;
    let (_, account_id) = crate::proxy::managed_accounts::selected(provider)?;
    let model = provider_config::selected_model(app, &provider.settings_config)
        .filter(|m| !m.is_empty())?;
    Some((account_id, model))
}
/// Runtime-only local availability classification after exact original reference validation.
/// Unknown/ambiguous identity or model conflicts are never classified as skippable.
pub(crate) fn runtime_local_unavailability(
    conn: &Connection,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    providers: &indexmap::IndexMap<String, Provider>,
    member: &super::routing_groups::Member,
    native: Option<&NativeReadGuard>,
) -> Result<Option<LocalNativeUnavailable>, AppError> {
    if member.source_id.starts_with("local:openrouter:") {
        let inspected =
            local_reference_on(conn, vault, &member.source_id)?.ok_or_else(source_unavailable)?;
        if inspected.fact.configured_model != member.model {
            return Err(source_unavailable());
        }
        super::openrouter_source::projection_for(&inspected.fact, app)?;
        return Ok(match inspected.availability {
            super::openrouter_source::Availability::Ready => None,
            super::openrouter_source::Availability::MissingCredential => {
                Some(LocalNativeUnavailable::AuthorizationAbsent)
            }
        });
    }
    if let Some(provider) = providers.get(&member.source_id) {
        if super::vendor_response_fence::owns(provider) {
            let source = binding_on(conn, app, provider)?.ok_or_else(source_unavailable)?;
            if source.source_id != member.source_id
                || !member_model_supported_on(conn, app, provider, &source, &member.model)?
            {
                return Err(source_unavailable());
            }
            return match super::vendor_response_fence::local_availability_on(conn, app, provider)
                .map_err(|_| source_unavailable())?
            {
                Some(super::vendor_response_fence::LocalKeyAvailability::Missing) => {
                    Ok(Some(LocalNativeUnavailable::AuthorizationAbsent))
                }
                _ => Ok(None),
            };
        }
    }
    if matches!(app, AppType::Codex) && qoder_source_namespace(&member.source_id) {
        let (_, identity) =
            qoder_identity_reference_on(conn, vault, member)?.ok_or_else(source_unavailable)?;
        return Ok(match identity.availability {
            crate::database::QoderRoutingSourceAvailability::Ready => None,
            crate::database::QoderRoutingSourceAvailability::RefreshPending => {
                Some(LocalNativeUnavailable::AuthorizationPending)
            }
            crate::database::QoderRoutingSourceAvailability::CredentialsExpired => {
                Some(LocalNativeUnavailable::CredentialsExpired)
            }
        });
    }
    let mut found = None;
    for provider in providers.values() {
        let Some(platform) = native_platform(provider) else {
            continue;
        };
        let Some((account, model)) = configured_native_projection(app, provider) else {
            continue;
        };
        if native_source_id(platform, &account, &model) != member.source_id {
            continue;
        }
        if model != member.model {
            return Err(AppError::Config("routing_groups.unavailable".into()));
        }
        if found.is_some() {
            return Err(AppError::Config(
                "routing_groups.ambiguous_native_source".into(),
            ));
        }
        found = Some((platform, account));
    }
    Ok(found.and_then(|(platform, account)| {
        native.and_then(|g| g.local_unavailability(platform, &account))
    }))
}
pub(crate) fn binding_with_native_on(
    conn: &Connection,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    provider: &Provider,
    native: Option<&NativeReadGuard>,
) -> Result<Option<NativeSourceBinding>, AppError> {
    if let Some(fact) = super::openrouter_source::read_on(
        conn,
        vault,
        &super::openrouter_source::RowRef {
            origin_app: app.as_str().into(),
            provider_id: provider.id.clone(),
        },
    )? {
        return Ok(Some(local_binding(&fact, app, &provider.id)));
    }
    if crate::proxy::qoder_http::enabled(provider) {
        if !matches!(app, AppType::Codex) {
            return Ok(None);
        }
        let Some(model) = provider_config::selected_model(app, &provider.settings_config)
            .filter(|m| !m.trim().is_empty() && m.trim() == m)
        else {
            return Ok(None);
        };
        let Some(fact) = crate::database::qoder_routing_source_on(
            conn,
            vault,
            provider,
            chrono::Utc::now().timestamp_millis(),
        )?
        else {
            return Ok(None);
        };
        return Ok(Some(NativeSourceBinding {
            source_id: native_source_id(fact.platform, &fact.account_id, &model),
            account: SourceAccountReference::NativeAccount {
                platform: fact.platform.into(),
                id: fact.account_id.clone(),
                region: None,
            },
            app_id: app.as_str().into(),
            provider_id: provider.id.clone(),
            model: Some(model),
            local_provider: None,
            native_stamp: Some(NativeStamp::Qoder(QoderStamp {
                fact,
                session_epoch: 0,
            })),
        }));
    }
    let Some(platform) = native_platform(provider) else {
        return binding_on(conn, app, provider);
    };
    let Some((account_id, model)) = configured_native_projection(app, provider) else {
        return Ok(None);
    };
    let Some(fact) = native.and_then(|guard| guard.account(platform, &account_id)) else {
        return Ok(None);
    };
    Ok(Some(NativeSourceBinding {
        source_id: native_source_id(platform, &account_id, &model),
        account: SourceAccountReference::NativeAccount {
            platform: platform.into(),
            id: account_id,
            region: match &fact.stamp {
                NativeStamp::MiniMax(stamp) => Some(stamp.region.clone()),
                NativeStamp::WorkBuddy(_) | NativeStamp::Qoder(_) => None,
                #[cfg(any(not(test), unix))]
                NativeStamp::ZCode(_) => None,
            },
        },
        app_id: app.as_str().into(),
        provider_id: provider.id.clone(),
        model: Some(model),
        local_provider: None,
        native_stamp: Some(fact.stamp.clone()),
    }))
}

fn local_binding(
    fact: &super::openrouter_source::SourceFact,
    app: &AppType,
    provider_id: &str,
) -> NativeSourceBinding {
    NativeSourceBinding {
        source_id: fact.source_id.clone(),
        account: SourceAccountReference::LocalProvider {
            platform: "openrouter".into(),
            id: fact.source_id.clone(),
            origin_app: fact.origin.origin_app.clone(),
            origin_provider_id: fact.origin.provider_id.clone(),
        },
        app_id: app.as_str().into(),
        provider_id: provider_id.into(),
        model: Some(fact.configured_model.clone()),
        native_stamp: None,
        local_provider: Some(fact.clone()),
    }
}

pub(crate) fn local_reference_on(
    conn: &Connection,
    vault: &crate::secrets::storage::StorageContext,
    source_id: &str,
) -> Result<Option<super::openrouter_source::Inspection>, AppError> {
    if !source_id.starts_with("local:openrouter:") {
        return Ok(None);
    }
    let mut statement =
        conn.prepare("SELECT id,app_type FROM providers WHERE app_type IN ('claude','codex')")?;
    let rows = statement.query_map([], |r| {
        Ok(super::openrouter_source::RowRef {
            provider_id: r.get(0)?,
            origin_app: r.get(1)?,
        })
    })?;
    let mut found = None;
    for row in rows {
        let row = row?;
        if super::openrouter_source::source_id_for(vault, &row)? != source_id {
            continue;
        }
        if found.is_some() {
            return Err(source_unavailable());
        }
        found = Some(
            super::openrouter_source::inspect_on(conn, vault, &row)?
                .ok_or_else(source_unavailable)?,
        );
    }
    Ok(found)
}

/// Original-row read for existing metadata consumers; not a grant to dispatch.
pub(crate) fn provider_for_local_reference_on(
    conn: &Connection,
    vault: &crate::secrets::storage::StorageContext,
    source_id: &str,
) -> Result<Option<Provider>, AppError> {
    let Some(inspected) = local_reference_on(conn, vault, source_id)? else {
        return Ok(None);
    };
    crate::database::Database::get_provider_by_id_on_connection(
        conn,
        vault,
        &inspected.fact.origin.provider_id,
        &inspected.fact.origin.origin_app,
    )
}

fn source_unavailable() -> AppError {
    AppError::Config("routing_groups.unavailable".into())
}
fn qoder_source_namespace(id: &str) -> bool {
    id.starts_with("native:qoder_sdk:") || id.starts_with("native:qoder_cn:")
}
fn qoder_platform(provider: &Provider) -> Option<&'static str> {
    use crate::services::provider::qoder_http_auth::SourceKind;
    match SourceKind::from_provider(provider).ok()? {
        SourceKind::InternationalQoderPat => Some("qoder_sdk"),
        SourceKind::ChinaQoderPat | SourceKind::ChinaQoderDevice => Some("qoder_cn"),
    }
}
/// Identity/capability is not a grant to send: Pending/expired retains only
/// original owner identity. Unknown/ambiguous/model-changed sources fail closed.
fn qoder_identity_reference_on(
    conn: &Connection,
    vault: &crate::secrets::storage::StorageContext,
    member: &super::routing_groups::Member,
) -> Result<Option<(Provider, crate::database::QoderRoutingIdentityFact)>, AppError> {
    let providers =
        crate::database::Database::get_all_providers_on_connection(conn, vault, "codex")?;
    let mut found = None;
    for provider in providers.values() {
        let Some(platform) = qoder_platform(provider) else {
            continue;
        };
        if !member.source_id.starts_with(&format!("native:{platform}:")) {
            continue;
        }
        let Some(model) =
            provider_config::selected_model(&AppType::Codex, &provider.settings_config)
                .filter(|m| !m.trim().is_empty() && m.trim() == m)
        else {
            continue;
        };
        let Some(identity) = crate::database::qoder_routing_identity_on(
            conn,
            vault,
            provider,
            chrono::Utc::now().timestamp_millis(),
        )?
        else {
            continue;
        };
        if native_source_id(platform, &identity.fact.account_id, &model) != member.source_id {
            continue;
        }
        if model != member.model {
            return Err(source_unavailable());
        }
        if found.is_some() {
            return Err(AppError::Config(
                "routing_groups.ambiguous_native_source".into(),
            ));
        }
        found = Some((provider.clone(), identity));
    }
    Ok(found)
}
pub(crate) enum MemberAppCapability {
    Compatible,
    UnsupportedApp(String),
}
pub(crate) fn member_app_capability(
    conn: &Connection,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    member: &super::routing_groups::Member,
) -> Result<MemberAppCapability, AppError> {
    if member.source_id.starts_with("local:openrouter:") {
        let inspected =
            local_reference_on(conn, vault, &member.source_id)?.ok_or_else(source_unavailable)?;
        if inspected.fact.configured_model != member.model {
            return Err(source_unavailable());
        }
        if super::openrouter_source::projection_for(&inspected.fact, app).is_err() {
            return Ok(MemberAppCapability::UnsupportedApp(
                inspected.fact.stable_evidence,
            ));
        }
        return Ok(MemberAppCapability::Compatible);
    }
    if matches!(app, AppType::Codex) || !qoder_source_namespace(&member.source_id) {
        return Ok(MemberAppCapability::Compatible);
    }
    let (provider, identity) =
        qoder_identity_reference_on(conn, vault, member)?.ok_or_else(source_unavailable)?;
    use sha2::{Digest, Sha256};
    let evidence = serde_json::to_vec(&(
        member,
        crate::database::Database::provider_update_digest(&provider)?,
        identity.fact,
    ))
    .map_err(|_| source_unavailable())?;
    Ok(MemberAppCapability::UnsupportedApp(hex::encode(
        Sha256::digest(evidence),
    )))
}
/// Row-local inventory diagnostics only, never runtime retry/authorization policy.
pub(crate) fn inventory_owner_unavailable(error: &AppError) -> bool {
    matches!(error,AppError::Config(code) if matches!(code.as_str(),
        "qoder.http_auth.invalid_record"|"qoder.http_auth.source_changed"|"qoder.http_auth.source_declaration_required"|
        "secret.unsupported_format"|"secret.invalid_envelope"|"secret.identity_mismatch"|"secret.authentication_failed"|"secret.invalid_text"))
}

pub(crate) fn read_native_for_sources<'a, 's>(
    session: &'a crate::secrets::session::SecretSession,
    vault: &crate::secrets::storage::StorageContext,
    sources: impl IntoIterator<Item = &'s str>,
) -> Result<Option<NativeReadGuard<'a>>, AppError> {
    let mut minimax = false;
    let mut workbuddy = false;
    let mut zcode = false;
    for id in sources {
        minimax |= id.starts_with("native:minimax_code:");
        workbuddy |= id.starts_with("native:workbuddy:");
        zcode |= id.starts_with("native:zcode_start_plan:");
    }
    if minimax || workbuddy || zcode {
        NativeReadGuard::acquire(session, vault, minimax, workbuddy, zcode).map(Some)
    } else {
        Ok(None)
    }
}
pub(crate) fn resolve_on(
    conn: &Connection,
    vault: &crate::secrets::storage::StorageContext,
    app: &AppType,
    providers: &indexmap::IndexMap<String, Provider>,
    source_id: &str,
    native: Option<&NativeReadGuard>,
) -> Result<Option<(Provider, NativeSourceBinding)>, AppError> {
    if source_id.starts_with("local:openrouter:") {
        let Some(inspected) = local_reference_on(conn, vault, source_id)? else {
            return Ok(None);
        };
        if inspected.availability != super::openrouter_source::Availability::Ready {
            return Ok(None);
        }
        let provider = super::openrouter_source::projection_for(&inspected.fact, app)?;
        let reference = local_binding(&inspected.fact, app, &provider.id);
        return Ok(Some((provider, reference)));
    }
    if !source_id.starts_with("native:minimax_code:")
        && !source_id.starts_with("native:workbuddy:")
        && !source_id.starts_with("native:zcode_start_plan:")
        && !source_id.starts_with("native:qoder_sdk:")
        && !source_id.starts_with("native:qoder_cn:")
    {
        let Some(provider) = providers.get(source_id) else {
            return Ok(None);
        };
        return Ok(binding_on(conn, app, provider)?.map(|binding| (provider.clone(), binding)));
    }
    let mut found = None;
    let wanted_platform = source_id.split(':').nth(1);
    for provider in providers.values().filter(|provider| {
        native_platform(provider).or_else(|| qoder_platform(provider)) == wanted_platform
    }) {
        let Some(binding) = binding_with_native_on(conn, vault, app, provider, native)? else {
            continue;
        };
        if binding.source_id != source_id {
            continue;
        }
        if found.is_some() {
            return Err(AppError::Config(
                "routing_groups.ambiguous_native_source".into(),
            ));
        }
        found = Some((provider.clone(), binding));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderMeta;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE loongport_relay(id INTEGER,site_origin TEXT,account_id INTEGER);\
             CREATE TABLE loongport_vendor(id INTEGER,vendor_id TEXT,account_id TEXT);",
        )
        .unwrap();
        conn
    }

    fn relay(account: Option<i64>) -> Provider {
        let mut provider = Provider::with_id(
            managed::provider_id_for("https://relay.example", account, 3),
            "Example".into(),
            serde_json::json!({"env":{"ANTHROPIC_MODEL":"model-a","ANTHROPIC_AUTH_TOKEN":"synthetic-secret"}}),
            Some("https://relay.example".into()),
        );
        provider.meta = Some(ProviderMeta {
            loongport_account_id: account,
            ..Default::default()
        });
        provider
    }

    #[test]
    fn relay_requires_recorded_account_and_site_and_rejects_ambiguity() {
        let conn = database();
        conn.execute(
            "INSERT INTO loongport_relay VALUES(11,'https://relay.example',77)",
            [],
        )
        .unwrap();
        assert!(binding_on(&conn, &AppType::Claude, &relay(None))
            .unwrap()
            .is_none());
        assert!(binding_on(&conn, &AppType::Claude, &relay(Some(88)))
            .unwrap()
            .is_none());
        let mut provider = relay(Some(77));
        let before = conn.total_changes();
        let binding = binding_on(&conn, &AppType::Claude, &provider)
            .unwrap()
            .unwrap();
        assert_eq!(binding.account, SourceAccountReference::Relay { id: 11 });
        assert_eq!(binding.model.as_deref(), Some("model-a"));
        assert_eq!(binding.source_id, provider.id);
        assert_eq!(binding.provider_id, provider.id);
        assert_eq!(binding.app_id, "claude");
        assert!(!format!("{binding:?}").contains("synthetic-secret"));
        assert_eq!(conn.total_changes(), before);
        provider.website_url = Some("https://unrelated.example".into());
        assert!(binding_on(&conn, &AppType::Claude, &provider)
            .unwrap()
            .is_none());
        conn.execute(
            "INSERT INTO loongport_relay VALUES(12,'https://relay.example',77)",
            [],
        )
        .unwrap();
        assert!(binding_on(&conn, &AppType::Claude, &relay(Some(77)))
            .unwrap()
            .is_none());
    }

    #[test]
    fn vendor_uses_existing_plan_identity_and_rejects_duplicate_owner() {
        let conn = database();
        conn.execute(
            "INSERT INTO loongport_vendor VALUES(21,'opencode','account-a')",
            [],
        )
        .unwrap();
        let vendor = crate::vendor::Vendor::OpenCode;
        for plan in crate::vendor::plans_for_account(vendor, Some("account-a")) {
            let provider = Provider::with_id(
                crate::vendor::provision::provider_id_for(plan.id_segment, "account-a"),
                "Plan".into(),
                serde_json::json!({}),
                None,
            );
            let binding = binding_on(&conn, &AppType::Codex, &provider)
                .unwrap()
                .unwrap();
            assert_eq!(binding.account, SourceAccountReference::Vendor { id: 21 });
            assert!(binding.model.is_none());
            assert!(binding_on(&conn, &AppType::Gemini, &provider)
                .unwrap()
                .is_none());
        }
        conn.execute(
            "INSERT INTO loongport_vendor VALUES(22,'opencode','account-a')",
            [],
        )
        .unwrap();
        let plan = &crate::vendor::plans_for_account(vendor, Some("account-a"))[0];
        let provider = Provider::with_id(
            crate::vendor::provision::provider_id_for(plan.id_segment, "account-a"),
            "Plan".into(),
            serde_json::json!({}),
            None,
        );
        assert!(vendor_source_binding::identity(&conn, &provider.id).is_err());
        assert!(binding_on(&conn, &AppType::Codex, &provider)
            .unwrap()
            .is_none());
    }

    #[test]
    fn identity_storage_failure_is_not_absence() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(binding_on(&conn, &AppType::Claude, &relay(Some(77))).is_err());
    }
}
