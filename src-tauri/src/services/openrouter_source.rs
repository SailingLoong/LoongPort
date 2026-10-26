//! Original SQL/vault OpenRouter row references; no account store or remote identity.
use crate::proxy::providers::{self, AuthStrategy};
use crate::{
    app_config::AppType,
    database::Database,
    error::AppError,
    provider::{AuthBindingSource, Provider},
    secrets::storage::StorageContext,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RowRef {
    pub origin_app: String,
    pub provider_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) enum WireProtocol {
    AnthropicMessages,
    OpenAiResponses,
    OpenAiChat,
}
/// Existing non-credential pricing overrides, not a new price or billing owner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PricingOverrides {
    pub cost_multiplier: Option<String>,
    pub pricing_model_source: Option<String>,
}
/// Internal fact; public projections must omit stable_evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceFact {
    pub source_id: String,
    pub origin: RowRef,
    pub name: String,
    pub configured_model: String,
    pub protocol: WireProtocol,
    pub stable_evidence: String,
    pub pricing: PricingOverrides,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Availability {
    Ready,
    MissingCredential,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inspection {
    pub fact: SourceFact,
    pub availability: Availability,
}
fn invalid() -> AppError {
    AppError::Config("openrouter.source_invalid".into())
}
fn opaque(domain: &[u8], value: serde_json::Value) -> Result<String, AppError> {
    let encoded = serde_json::to_vec(&value).map_err(|_| invalid())?;
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(encoded);
    Ok(hex::encode(hash.finalize()))
}
pub(crate) fn source_id_for(vault: &StorageContext, origin: &RowRef) -> Result<String, AppError> {
    Ok(format!(
        "local:openrouter:{}",
        opaque(
            b"loongport.openrouter-source.v1\0",
            serde_json::json!([
                vault.identity().store_id(),
                origin.origin_app,
                origin.provider_id
            ])
        )?
    ))
}
fn official_base(raw: &str, protocol: WireProtocol) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str() == Some("openrouter.ai")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.query().is_none()
        && url.fragment().is_none()
        && match protocol {
            WireProtocol::AnthropicMessages => {
                matches!(url.path(), "/api" | "/api/" | "/api/v1" | "/api/v1/")
            }
            _ => matches!(url.path(), "/api/v1" | "/api/v1/"),
        }
}
/// Use the original adapter's effective upstream format. A client wire hint
/// or shadowed compatibility field is not a second protocol constraint.
fn protocol(app: &AppType, provider: &Provider) -> Result<Option<WireProtocol>, AppError> {
    if matches!(app, AppType::Claude) {
        return Ok(match providers::get_claude_api_format(provider) {
            "anthropic" => Some(WireProtocol::AnthropicMessages),
            "openai_chat" => Some(WireProtocol::OpenAiChat),
            "openai_responses" => Some(WireProtocol::OpenAiResponses),
            _ => None,
        });
    }
    let effective = if providers::codex_provider_uses_anthropic(provider) {
        WireProtocol::AnthropicMessages
    } else if providers::codex_provider_uses_chat_completions(provider) {
        WireProtocol::OpenAiChat
    } else {
        WireProtocol::OpenAiResponses
    };
    // Independently validate the original native destination, not its client
    // wire declaration. The proxy's effective base is checked by inspect_on.
    let Some(text) = provider
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
    else {
        return Ok(None);
    };
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| invalid())?;
    let Some(name) = doc.get("model_provider").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let Some(table) = doc.get("model_providers").and_then(|v| v.get(name)) else {
        return Ok(None);
    };
    if !table
        .get("base_url")
        .and_then(|v| v.as_str())
        .is_some_and(|b| official_base(b, effective))
    {
        return Ok(None);
    }
    Ok(Some(effective))
}
/// Caller holds original vault/connection. No discovery, locks, writes or HTTP.
/// Only an explicit empty credential in an otherwise verified row is typed missing.
pub(crate) fn inspect_on(
    conn: &Connection,
    vault: &StorageContext,
    origin: &RowRef,
) -> Result<Option<Inspection>, AppError> {
    let app = match origin.origin_app.as_str() {
        "claude" => AppType::Claude,
        "codex" => AppType::Codex,
        _ => return Ok(None),
    };
    let Some(provider) = Database::get_provider_by_id_on_connection(
        conn,
        vault,
        &origin.provider_id,
        &origin.origin_app,
    )?
    else {
        return Ok(None);
    };
    if let Some(meta) = &provider.meta {
        if !meta.custom_endpoints.is_empty()
            || meta.provider_type.is_some()
            || meta.loongport_vendor_account.is_some()
            || meta.loongport_account_id.is_some()
            || meta
                .auth_binding
                .as_ref()
                .is_some_and(|b| b.source != AuthBindingSource::ProviderConfig)
        {
            return Ok(None);
        }
    }
    let Some(protocol) = protocol(&app, &provider)? else {
        return Ok(None);
    };
    let adapter = providers::get_adapter(&app).ok_or_else(invalid)?;
    let base = adapter.extract_base_url(&provider).map_err(|_| invalid())?;
    if !official_base(&base, protocol) {
        return Ok(None);
    }
    let Some(model) =
        crate::relay::provider_config::selected_model(&app, &provider.settings_config)
            .filter(|m| !m.trim().is_empty() && m.trim() == m)
    else {
        return Ok(None);
    };
    let availability = match adapter.extract_auth(&provider) {
        Some(auth) => {
            if auth.strategy != AuthStrategy::Bearer || auth.access_token.is_some() {
                return Ok(None);
            }
            adapter.get_auth_headers(&auth).map_err(|_| invalid())?;
            if auth.api_key.trim().is_empty() {
                Availability::MissingCredential
            } else {
                Availability::Ready
            }
        }
        None => {
            let paths: &[&[&str]] = if matches!(app, AppType::Codex) {
                &[&["auth", "OPENAI_API_KEY"]]
            } else {
                &[
                    &["env", "ANTHROPIC_AUTH_TOKEN"],
                    &["env", "ANTHROPIC_API_KEY"],
                    &["env", "OPENROUTER_API_KEY"],
                    &["env", "OPENAI_API_KEY"],
                ]
            };
            let values = paths
                .iter()
                .filter_map(|path| {
                    path.iter()
                        .try_fold(&provider.settings_config, |v, key| v.get(*key))
                })
                .collect::<Vec<_>>();
            if values.is_empty()
                || values
                    .iter()
                    .any(|v| v.as_str().is_none_or(|key| !key.trim().is_empty()))
            {
                return Err(invalid());
            }
            Availability::MissingCredential
        }
    };
    let revision = crate::database::credential_revision::provider_on(
        conn,
        &origin.origin_app,
        &origin.provider_id,
    )?;
    // Bind original auth material as well as issuance identity; labels and model
    // preferences are not a new key and cannot retire a response already in flight.
    let auth = adapter.extract_auth(&provider);
    let credential = auth
        .as_ref()
        .map(|auth| (&auth.api_key, &auth.access_token));
    let source_id = source_id_for(vault, origin)?;
    let stable_evidence = opaque(
        b"loongport.openrouter-evidence.v1\0",
        serde_json::json!([source_id, revision, credential, base, protocol]),
    )?;
    Ok(Some(Inspection {
        fact: SourceFact {
            source_id,
            origin: origin.clone(),
            name: provider.name,
            configured_model: model,
            protocol,
            stable_evidence,
            pricing: PricingOverrides {
                cost_multiplier: provider
                    .meta
                    .as_ref()
                    .and_then(|m| m.cost_multiplier.clone()),
                pricing_model_source: provider
                    .meta
                    .as_ref()
                    .and_then(|m| m.pricing_model_source.clone()),
            },
        },
        availability,
    }))
}
pub(crate) fn read_on(
    conn: &Connection,
    vault: &StorageContext,
    origin: &RowRef,
) -> Result<Option<SourceFact>, AppError> {
    Ok(inspect_on(conn, vault, origin)?
        .and_then(|i| (i.availability == Availability::Ready).then_some(i.fact)))
}

/// Private request snapshot. Never serialized or logged.
#[derive(Clone)]
pub(crate) struct Expected {
    inspection: Inspection,
    storage: crate::secrets::storage::StorageIdentity,
    epoch: u64,
}
impl Expected {
    pub(crate) fn fact(&self) -> &SourceFact {
        &self.inspection.fact
    }
    pub(crate) fn availability(&self) -> Availability {
        self.inspection.availability
    }
    pub(crate) fn origin(&self) -> &RowRef {
        &self.inspection.fact.origin
    }
}
pub(crate) fn capture_on(
    conn: &Connection,
    vault: &StorageContext,
    epoch: u64,
    origin: &RowRef,
) -> Result<Expected, AppError> {
    Ok(Expected {
        inspection: inspect_on(conn, vault, origin)?.ok_or_else(invalid)?,
        storage: vault.identity(),
        epoch,
    })
}
/// Original adapters consume this in-memory projection; never save it as a row.
pub(crate) fn projection_for(fact: &SourceFact, target: &AppType) -> Result<Provider, AppError> {
    if !matches!(target, AppType::Claude | AppType::Codex) {
        return Err(invalid());
    }
    let settings = crate::relay::provider_config::settings_config_with_roles_and_models(
        target,
        "",
        &fact.name,
        "https://openrouter.ai/api/v1",
        &fact.configured_model,
        None,
        None,
        crate::relay::provider_config::ProvisionStyle {
            codex_wire_chat: fact.protocol == WireProtocol::OpenAiChat,
            ..Default::default()
        },
    )
    .ok_or_else(invalid)?;
    let mut provider = Provider::with_id(
        fact.source_id.clone(),
        fact.name.clone(),
        settings,
        Some("https://openrouter.ai".into()),
    );
    provider.category = Some("aggregator".into());
    provider.meta = Some(crate::provider::ProviderMeta {
        cost_multiplier: fact.pricing.cost_multiplier.clone(),
        pricing_model_source: fact.pricing.pricing_model_source.clone(),
        api_format: Some(
            match fact.protocol {
                WireProtocol::AnthropicMessages => "anthropic",
                WireProtocol::OpenAiResponses => "openai_responses",
                WireProtocol::OpenAiChat => "openai_chat",
            }
            .into(),
        ),
        ..Default::default()
    });
    Ok(provider)
}
#[derive(Clone)]
pub(crate) struct Fence {
    db: std::sync::Arc<Database>,
    expected: Expected,
}
impl Fence {
    pub(crate) fn check(&self) -> Result<(), AppError> {
        let vault = self.db.secrets.read()?;
        let mut conn = self.db.conn.lock()?;
        let tx = conn.transaction()?;
        validate_expected_on(&self.db, &tx, &vault, &self.expected)?;
        check_epoch(&self.db, &self.expected)
    }
}
fn check_epoch(db: &Database, expected: &Expected) -> Result<(), AppError> {
    db.secrets.ensure_available()?;
    if db.secrets.lifecycle_epoch() != expected.epoch {
        return Err(invalid());
    }
    Ok(())
}
fn validate_expected_on(
    db: &Database,
    conn: &Connection,
    vault: &StorageContext,
    expected: &Expected,
) -> Result<(), AppError> {
    check_epoch(db, expected)?;
    if vault.identity() != expected.storage {
        return Err(invalid());
    }
    let current = inspect_on(conn, vault, expected.origin())?.ok_or_else(invalid)?;
    if current.fact.stable_evidence != expected.inspection.fact.stable_evidence
        || current.fact.source_id != expected.inspection.fact.source_id
        || current.availability != Availability::Ready
    {
        return Err(invalid());
    }
    check_epoch(db, expected)
}
/// Validate captured owner and resolve its credential in the SAME original read transaction.
/// Never replaces a retired request's evidence with a fresh snapshot.
pub(crate) fn acquire_expected(
    db: std::sync::Arc<Database>,
    target: &AppType,
    expected: &Expected,
) -> Result<(Provider, Fence), AppError> {
    let projection = {
        let vault = db.secrets.read()?;
        let mut conn = db.conn.lock()?;
        let tx = conn.transaction()?;
        validate_expected_on(&db, &tx, &vault, expected)?;
        let row = Database::get_provider_by_id_on_connection(
            &tx,
            &vault,
            &expected.origin().provider_id,
            &expected.origin().origin_app,
        )?
        .ok_or_else(invalid)?;
        let app = expected
            .origin()
            .origin_app
            .parse::<AppType>()
            .map_err(|_| invalid())?;
        let adapter = providers::get_adapter(&app).ok_or_else(invalid)?;
        let mut auth = adapter.extract_auth(&row).ok_or_else(invalid)?;
        let mut projection = projection_for(expected.fact(), target)?;
        if let Some(original) = row.meta.as_ref() {
            let transient = projection.meta.as_mut().ok_or_else(invalid)?;
            // Preserve original request policy only. Destination, account binding,
            // lifecycle state and client/native projection metadata are not copied.
            transient.application_model_overrides = original.application_model_overrides.clone();
            transient.prompt_cache_key = original.prompt_cache_key.clone();
            transient.prompt_cache_routing = original.prompt_cache_routing.clone();
            transient.codex_chat_reasoning = original.codex_chat_reasoning.clone();
            transient.impersonate_claude_code = original.impersonate_claude_code;
            transient.max_output_tokens = original.max_output_tokens;
            transient.custom_user_agent = original.custom_user_agent.clone();
            transient.local_proxy_request_overrides =
                original.local_proxy_request_overrides.clone();
        }
        let patched = crate::relay::provider_config::patch_api_key(
            &mut projection.settings_config,
            target,
            &auth.api_key,
        );
        use zeroize::Zeroize;
        auth.api_key.zeroize();
        if !patched {
            return Err(invalid());
        }
        check_epoch(&db, expected)?;
        projection
    };
    Ok((
        projection,
        Fence {
            db,
            expected: expected.clone(),
        },
    ))
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "openrouter_source_tests.rs"]
mod source_tests;
#[cfg(feature = "test-hooks")]
pub(crate) fn verify_fixture(case: &str) {
    source_tests::verify(case);
}
