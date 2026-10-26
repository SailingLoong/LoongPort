//! Synthetic original encrypted Provider owner only; no public HTTP or account use.
use super::*;
use crate::{app_config::AppType, database::Database, provider::Provider};

fn add(db: &Database, app: &str, id: &str, base: &str, key: &str) -> RowRef {
    let app_type = app.parse::<AppType>().unwrap();
    let settings = crate::relay::provider_config::settings_config_for(
        &app_type,
        key,
        "Synthetic OpenRouter",
        base,
        "synthetic/model",
    )
    .unwrap();
    let provider = Provider::with_id(
        id.into(),
        "Synthetic OpenRouter".into(),
        settings,
        Some("https://openrouter.ai".into()),
    );
    db.save_provider(app, &provider).unwrap();
    RowRef {
        origin_app: app.into(),
        provider_id: id.into(),
    }
}
fn read(db: &Database, row: &RowRef) -> Result<Option<SourceFact>, AppError> {
    let vault = db.secrets.read()?;
    let conn = db.conn.lock()?;
    let changes = conn.total_changes();
    let result = read_on(&conn, &vault, row);
    assert_eq!(
        changes,
        conn.total_changes(),
        "source read must not mutate original owner"
    );
    result
}

#[cfg_attr(test, test)]
fn original_rows_produce_opaque_credential_free_facts() {
    let db = Database::memory().unwrap();
    for (app, base, protocol) in [
        (
            "claude",
            "https://openrouter.ai/api",
            WireProtocol::AnthropicMessages,
        ),
        (
            "codex",
            "https://openrouter.ai/api/v1",
            WireProtocol::OpenAiResponses,
        ),
    ] {
        let row = add(&db, app, "same-local-id", base, "synthetic-original-secret");
        let fact = read(&db, &row)
            .unwrap()
            .expect("original official row must be visible");
        assert_eq!(fact.origin, row);
        assert_eq!(fact.configured_model, "synthetic/model");
        assert_eq!(fact.protocol, protocol);
        assert!(!fact.source_id.contains("same-local-id"));
        assert!(!fact.source_id.contains("synthetic-original-secret"));
        assert!(!fact.stable_evidence.contains("synthetic-original-secret"));
    }
    let claude = RowRef {
        origin_app: "claude".into(),
        provider_id: "same-local-id".into(),
    };
    let codex = RowRef {
        origin_app: "codex".into(),
        provider_id: "same-local-id".into(),
    };
    assert_ne!(
        read(&db, &claude).unwrap().unwrap().source_id,
        read(&db, &codex).unwrap().unwrap().source_id,
        "same id in another app is not the same credential owner"
    );
}

// Match the real Claude Official preset: ordinary local config has no endpoint.
// The original overview must not mistake this for a damaged OpenRouter owner.
#[cfg(feature = "gui")]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn default_claude_official_keeps_original_overview_and_sources_readable() {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let db = std::sync::Arc::new(Database::memory().unwrap());
    let mut provider = Provider::with_id(
        "synthetic-default-claude".into(),
        "Claude Official".into(),
        serde_json::json!({"env": {}}),
        Some("https://www.anthropic.com/claude-code".into()),
    );
    provider.category = Some("official".into());
    db.save_provider("claude", &provider).unwrap();
    let state = crate::store::AppState::new(db.clone()).unwrap();
    let overview = crate::commands::application_overview(&state, &AppType::Claude)
        .expect("default Claude is not an OpenRouter owner");
    let displayed = overview
        .configurations
        .iter()
        .find(|row| row.provider_id == provider.id)
        .expect("the original default provider must remain visible");
    assert!(displayed.account.is_none());
    assert!(crate::commands::get_service_sources_impl(&state)
        .unwrap()
        .is_empty());
    assert!(read(
        &db,
        &RowRef {
            origin_app: "claude".into(),
            provider_id: provider.id,
        }
    )
    .unwrap()
    .is_none());
}

#[cfg_attr(test, test)]
fn malformed_endpoint_fields_and_original_credentials_remain_errors() {
    let db = Database::memory().unwrap();
    for (index, settings) in [
        serde_json::json!({"env": {"ANTHROPIC_BASE_URL": 7}}),
        serde_json::json!({"env": {"ANTHROPIC_BASE_URL": null}}),
        serde_json::json!({"env": []}),
        serde_json::json!({"env": {}, "base_url": false}),
        serde_json::json!({"env": {}, "baseURL": null}),
        serde_json::json!({"env": {}, "apiEndpoint": {}}),
    ]
    .into_iter()
    .enumerate()
    {
        let row = add(
            &db,
            "claude",
            &format!("malformed-endpoint-{index}"),
            "https://openrouter.ai/api",
            "synthetic-secret",
        );
        let mut provider = db
            .get_provider_by_id(&row.provider_id, "claude")
            .unwrap()
            .unwrap();
        provider.settings_config = settings;
        db.save_provider("claude", &provider).unwrap();
        assert!(
            read(&db, &row).is_err(),
            "damaged fields are not absent fields"
        );
    }
    for (index, key) in [
        serde_json::json!("synthetic\ninvalid"),
        serde_json::json!(7),
    ]
    .into_iter()
    .enumerate()
    {
        let row = add(
            &db,
            "claude",
            &format!("malformed-auth-{index}"),
            "https://openrouter.ai/api",
            "synthetic-secret",
        );
        let mut provider = db
            .get_provider_by_id(&row.provider_id, "claude")
            .unwrap()
            .unwrap();
        provider.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"] = key;
        db.save_provider("claude", &provider).unwrap();
        assert!(
            read(&db, &row).is_err(),
            "verified OpenRouter auth corruption is not absence"
        );
    }
}

#[cfg_attr(test, test)]
fn captured_openrouter_endpoint_removal_retires_request_and_response() {
    let db = std::sync::Arc::new(Database::memory().unwrap());
    let row = add(
        &db,
        "claude",
        "removed-endpoint",
        "https://openrouter.ai/api",
        "synthetic-secret",
    );
    let expected = capture(&db, &row);
    let (_, fence) = acquire_expected(db.clone(), &AppType::Claude, &expected).unwrap();
    let mut provider = db
        .get_provider_by_id(&row.provider_id, "claude")
        .unwrap()
        .unwrap();
    provider.settings_config["env"]
        .as_object_mut()
        .unwrap()
        .remove("ANTHROPIC_BASE_URL");
    db.save_provider("claude", &provider).unwrap();
    assert!(acquire_expected(db.clone(), &AppType::Claude, &expected).is_err());
    assert!(fence.check().is_err());
    {
        let vault = db.secrets.read().unwrap();
        let conn = db.conn.lock().unwrap();
        assert!(capture_on(&conn, &vault, db.secrets.lifecycle_epoch(), &row).is_err());
    }
    // A fresh inventory read may classify absence, but never revive a proof.
    assert!(read(&db, &row).unwrap().is_none());
}

#[cfg_attr(test, test)]
fn exact_official_destination_is_required_not_brand_or_substring() {
    let db = Database::memory().unwrap();
    for (i, base) in [
        "https://openrouter.ai.evil.invalid/api",
        "https://evil.invalid/openrouter.ai/api",
        "http://openrouter.ai/api",
        "https://user@openrouter.ai/api",
        "https://openrouter.ai:444/api",
        "https://openrouter.ai/api?redirect=1",
        "https://openrouter.ai/api#fragment",
        "https://openrouter.ai/other",
    ]
    .iter()
    .enumerate()
    {
        let row = add(&db, "claude", &format!("bad-{i}"), base, "synthetic-secret");
        assert!(
            read(&db, &row).map(|f| f.is_none()).unwrap_or(true),
            "not an official source: {base}"
        );
    }
}

#[cfg_attr(test, test)]
fn normal_reseal_retires_evidence_not_the_local_source_reference() {
    let db = Database::memory().unwrap();
    let row = add(
        &db,
        "codex",
        "reseal",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let before = read(&db, &row).unwrap().unwrap();
    let provider = db
        .get_provider_by_id(&row.provider_id, &row.origin_app)
        .unwrap()
        .unwrap();
    db.save_provider_with_credential_intent(&row.origin_app, &provider, true)
        .unwrap();
    let after = read(&db, &row).unwrap().unwrap();
    assert_eq!(before.source_id, after.source_id);
    assert_ne!(
        before.stable_evidence, after.stable_evidence,
        "same plaintext reseal is a new original-row incarnation"
    );
}

#[cfg_attr(test, test)]
fn missing_and_empty_are_not_sources_but_corruption_is_not_absence() {
    let db = Database::memory().unwrap();
    let missing = RowRef {
        origin_app: "codex".into(),
        provider_id: "missing".into(),
    };
    assert!(read(&db, &missing).unwrap().is_none());
    let row = add(&db, "codex", "empty", "https://openrouter.ai/api/v1", "");
    assert!(read(&db, &row).unwrap().is_none());
    db.conn.lock().unwrap().execute("UPDATE providers SET settings_config='nonempty-malformed-envelope' WHERE id='empty' AND app_type='codex'",[]).unwrap();
    assert!(
        read(&db, &row).is_err(),
        "corrupt original credential owner is not a missing key"
    );
}

#[cfg_attr(test, test)]
fn custom_endpoints_managed_bindings_and_unknown_formats_are_not_sources() {
    let db = Database::memory().unwrap();
    for kind in ["endpoint", "managed", "format"] {
        let row = add(
            &db,
            "claude",
            kind,
            "https://openrouter.ai/api",
            "synthetic-secret",
        );
        let mut provider = db.get_provider_by_id(kind, "claude").unwrap().unwrap();
        let meta = provider.meta.get_or_insert_with(Default::default);
        match kind {
            "endpoint" => {
                meta.custom_endpoints.insert(
                    "https://evil.invalid".into(),
                    crate::settings::CustomEndpoint {
                        url: "https://evil.invalid".into(),
                        added_at: 1,
                        last_used: None,
                    },
                );
            }
            "managed" => {
                meta.auth_binding = Some(crate::provider::AuthBinding {
                    source: crate::provider::AuthBindingSource::ManagedAccount,
                    auth_provider: Some("unrelated".into()),
                    account_id: Some("synthetic-account".into()),
                });
            }
            _ => {
                meta.api_format = Some("gemini_native".into());
            }
        }
        db.save_provider("claude", &provider).unwrap();
        if kind == "endpoint" {
            // Existing-row save does not own endpoint edits; use its original owner.
            db.add_custom_endpoint("claude", kind, "https://evil.invalid")
                .unwrap();
            assert!(!db
                .get_provider_by_id(kind, "claude")
                .unwrap()
                .unwrap()
                .meta
                .unwrap()
                .custom_endpoints
                .is_empty());
        }
        assert!(
            read(&db, &row).map(|f| f.is_none()).unwrap_or(true),
            "unsupported owner/config: {kind}"
        );
    }
}

fn capture(db: &Database, row: &RowRef) -> Expected {
    let vault = db.secrets.read().unwrap();
    let conn = db.conn.lock().unwrap();
    capture_on(&conn, &vault, db.secrets.lifecycle_epoch(), row).unwrap()
}
#[cfg_attr(test, test)]
fn projection_is_transient_and_preserves_original_wire_and_owner_id() {
    let db = Database::memory().unwrap();
    let row = add(
        &db,
        "codex",
        "projection",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let expected = capture(&db, &row);
    for target in [AppType::Claude, AppType::Codex] {
        let projected = projection_for(expected.fact(), &target).unwrap();
        assert_eq!(projected.id, expected.fact().source_id);
        assert_eq!(
            crate::relay::provider_config::selected_model(&target, &projected.settings_config)
                .as_deref(),
            Some("synthetic/model")
        );
        assert!(crate::relay::provider_config::extract_api_key(
            &projected.settings_config,
            &target
        )
        .is_none_or(|k| k.is_empty()));
    }
    assert!(db.get_all_providers("claude").unwrap().is_empty());
    assert_eq!(db.get_all_providers("codex").unwrap().len(), 1);
}
#[cfg_attr(test, test)]
fn captured_request_rejects_same_key_reseal_while_new_request_remains_usable() {
    let db = std::sync::Arc::new(Database::memory().unwrap());
    let row = add(
        &db,
        "codex",
        "expected-reseal",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let old = capture(&db, &row);
    let provider = db
        .get_provider_by_id(&row.provider_id, &row.origin_app)
        .unwrap()
        .unwrap();
    db.save_provider_with_credential_intent(&row.origin_app, &provider, true)
        .unwrap();
    let new = capture(&db, &row);
    let (projection, _) = acquire_expected(db.clone(), &AppType::Claude, &new).unwrap();
    assert_eq!(
        crate::relay::provider_config::extract_api_key(
            &projection.settings_config,
            &AppType::Claude
        )
        .as_deref(),
        Some("synthetic-secret")
    );
    assert!(
        acquire_expected(db, &AppType::Claude, &old).is_err(),
        "old request cannot adopt reissued original credentials"
    );
}
#[cfg_attr(test, test)]
fn acquired_fence_rejects_key_reissue() {
    let db = std::sync::Arc::new(Database::memory().unwrap());
    let row = add(
        &db,
        "codex",
        "fence-key",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let expected = capture(&db, &row);
    let (_, fence) = acquire_expected(db.clone(), &AppType::Codex, &expected).unwrap();
    fence.check().unwrap();
    let provider = db
        .get_provider_by_id(&row.provider_id, &row.origin_app)
        .unwrap()
        .unwrap();
    db.save_provider_with_credential_intent(&row.origin_app, &provider, true)
        .unwrap();
    assert!(
        fence.check().is_err(),
        "final output cannot survive original-row reissue"
    );
}
#[cfg_attr(test, test)]
fn captured_and_acquired_leases_do_not_revive_after_session_unblock() {
    let db = std::sync::Arc::new(Database::memory().unwrap());
    let row = add(
        &db,
        "codex",
        "epoch",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let expected = capture(&db, &row);
    let (_, fence) = acquire_expected(db.clone(), &AppType::Codex, &expected).unwrap();
    db.secrets.set_blocked(true);
    db.secrets.set_blocked(false);
    let fresh = capture(&db, &row);
    acquire_expected(db.clone(), &AppType::Codex, &fresh).unwrap();
    assert!(
        fence.check().is_err(),
        "old output lease must not revive on unlock"
    );
    assert!(
        acquire_expected(db, &AppType::Codex, &expected).is_err(),
        "captured old request must not revive on unlock"
    );
}

#[cfg_attr(test, test)]
fn protocol_aliases_follow_original_effective_precedence() {
    let db = Database::memory().unwrap();
    for (i, kind) in [
        "camel",
        "shadowed",
        "numeric",
        "root_wire",
        "claude_shadowed",
    ]
    .iter()
    .enumerate()
    {
        let app = if *kind == "claude_shadowed" {
            "claude"
        } else {
            "codex"
        };
        let row = add(
            &db,
            app,
            &format!("alias-{i}"),
            "https://openrouter.ai/api/v1",
            "synthetic-secret",
        );
        let mut provider = db
            .get_provider_by_id(&row.provider_id, app)
            .unwrap()
            .unwrap();
        match *kind {
            "camel" => provider.settings_config["apiFormat"] = serde_json::json!("openai_chat"),
            "shadowed" => {
                provider
                    .meta
                    .get_or_insert_with(Default::default)
                    .api_format = Some("openai_responses".into());
                provider.settings_config["api_format"] = serde_json::json!("openai_chat");
            }
            "numeric" => provider.settings_config["apiFormat"] = serde_json::json!(42),
            "root_wire" => {
                let current = provider.settings_config["config"].as_str().unwrap();
                provider.settings_config["config"] =
                    serde_json::json!(format!("wire_api = \"chat\"\n{current}"));
            }
            _ => {
                provider
                    .meta
                    .get_or_insert_with(Default::default)
                    .api_format = Some("anthropic".into());
                provider.settings_config["api_format"] = serde_json::json!("openai_chat");
            }
        }
        db.save_provider(app, &provider).unwrap();
        let expected = match *kind {
            "camel" => WireProtocol::OpenAiChat,
            "claude_shadowed" => WireProtocol::AnthropicMessages,
            _ => WireProtocol::OpenAiResponses,
        };
        assert_eq!(
            read(&db, &row)
                .unwrap()
                .expect("original effective protocol remains supported")
                .protocol,
            expected,
            "shared source must follow original adapter precedence: {kind}"
        );
    }
}

#[cfg_attr(test, test)]
fn acquisition_preserves_original_request_options_without_owner_or_destination_metadata() {
    let db = std::sync::Arc::new(Database::memory().unwrap());
    let row = add(
        &db,
        "codex",
        "request-options",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let mut provider = db
        .get_provider_by_id(&row.provider_id, &row.origin_app)
        .unwrap()
        .unwrap();
    let meta = provider.meta.get_or_insert_with(Default::default);
    meta.max_output_tokens = Some(12345);
    meta.custom_user_agent = Some("SyntheticOwner/1".into());
    meta.prompt_cache_key = Some("synthetic-cache-key".into());
    meta.prompt_cache_routing = Some("disabled".into());
    meta.local_proxy_request_overrides = Some(crate::provider::LocalProxyRequestOverrides {
        headers: std::collections::HashMap::from([(
            "X-Synthetic-Option".into(),
            "preserved".into(),
        )]),
        body: Some(serde_json::json!({"temperature":0.2})),
    });
    db.save_provider(&row.origin_app, &provider).unwrap();
    let expected = capture(&db, &row);
    let (projection, _) = acquire_expected(db.clone(), &AppType::Claude, &expected).unwrap();
    let actual = projection.meta.unwrap();
    assert_eq!(actual.max_output_tokens, Some(12345));
    assert_eq!(
        actual.custom_user_agent.as_deref(),
        Some("SyntheticOwner/1")
    );
    assert_eq!(
        actual.prompt_cache_key.as_deref(),
        Some("synthetic-cache-key")
    );
    assert_eq!(actual.prompt_cache_routing.as_deref(), Some("disabled"));
    let overrides = actual.local_proxy_request_overrides.unwrap();
    assert_eq!(
        overrides
            .headers
            .get("X-Synthetic-Option")
            .map(String::as_str),
        Some("preserved")
    );
    assert_eq!(overrides.body, Some(serde_json::json!({"temperature":0.2})));
    assert!(
        actual.auth_binding.is_none()
            && actual.provider_type.is_none()
            && actual.custom_endpoints.is_empty()
    );
    assert!(db.get_all_providers("claude").unwrap().is_empty());
}

#[cfg_attr(test, test)]
fn credential_free_projection_keeps_original_pricing_overrides() {
    let db = Database::memory().unwrap();
    let row = add(
        &db,
        "codex",
        "pricing-overrides",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let mut provider = db
        .get_provider_by_id(&row.provider_id, &row.origin_app)
        .unwrap()
        .unwrap();
    let meta = provider.meta.get_or_insert_with(Default::default);
    meta.cost_multiplier = Some("1.25".into());
    meta.pricing_model_source = Some("request".into());
    db.save_provider(&row.origin_app, &provider).unwrap();
    let fact = read(&db, &row).unwrap().unwrap();
    for target in [AppType::Claude, AppType::Codex] {
        let projected = projection_for(&fact, &target).unwrap();
        let meta = projected.meta.unwrap();
        assert_eq!(meta.cost_multiplier.as_deref(), Some("1.25"));
        assert_eq!(meta.pricing_model_source.as_deref(), Some("request"));
        assert!(crate::relay::provider_config::extract_api_key(
            &projected.settings_config,
            &target
        )
        .is_none_or(|key| key.is_empty()));
    }
    assert!(db.get_all_providers("claude").unwrap().is_empty());
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
fn plaintext_credential_retirement(case: &str) {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let db = std::sync::Arc::new(
        crate::services::native_source_guard::storage_fixture::fresh_plaintext_database(),
    );
    let row = add(
        &db,
        "codex",
        "plaintext-original",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let old = capture(&db, &row);
    let (_, fence) = acquire_expected(db.clone(), &AppType::Codex, &old).unwrap();
    let provider = db
        .get_provider_by_id(&row.provider_id, &row.origin_app)
        .unwrap()
        .unwrap();
    if case.ends_with("_aba") {
        let mut empty = provider.clone();
        empty.settings_config["auth"]["OPENAI_API_KEY"] = serde_json::Value::String(String::new());
        db.save_provider(&row.origin_app, &empty).unwrap();
        assert!(acquire_expected(db.clone(), &AppType::Codex, &old).is_err());
        assert!(fence.check().is_err());
    }
    db.save_provider_with_credential_intent(&row.origin_app, &provider, true)
        .unwrap();
    let fresh = capture(&db, &row);
    acquire_expected(db.clone(), &AppType::Codex, &fresh).unwrap();
    if case.starts_with("plaintext_response_") {
        assert!(
            fence.check().is_err(),
            "plaintext original-row reissue must retire final-response proof"
        );
    } else {
        assert!(
            acquire_expected(db, &AppType::Codex, &old).is_err(),
            "plaintext original-row reissue must retire captured request proof"
        );
    }
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
fn plaintext_noncredential_or_recreate(case: &str) {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let db = std::sync::Arc::new(
        crate::services::native_source_guard::storage_fixture::fresh_plaintext_database(),
    );
    let row = add(
        &db,
        "codex",
        "preserved-source",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let expected = capture(&db, &row);
    let (_, fence) = acquire_expected(db.clone(), &AppType::Codex, &expected).unwrap();
    let mut provider = db
        .get_provider_by_id(&row.provider_id, &row.origin_app)
        .unwrap()
        .unwrap();
    if case == "plaintext_recreate" {
        db.delete_provider(&row.origin_app, &row.provider_id)
            .unwrap();
        db.save_provider(&row.origin_app, &provider).unwrap();
        assert!(acquire_expected(db, &AppType::Codex, &expected).is_err());
        assert!(fence.check().is_err());
        return;
    }
    provider.name = "Renamed local account".into();
    if case == "plaintext_model_preserves" {
        provider.settings_config["config"] = serde_json::json!(provider.settings_config["config"]
            .as_str()
            .unwrap()
            .replace("synthetic/model", "synthetic/new-model"));
    }
    db.save_provider(&row.origin_app, &provider).unwrap();
    acquire_expected(db, &AppType::Codex, &expected).unwrap();
    fence.check().unwrap();
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
fn plaintext_sql_replacement_retires_original_leases() {
    let _home = crate::secrets::testing::TestHome::new().unwrap();
    let db = std::sync::Arc::new(
        crate::services::native_source_guard::storage_fixture::fresh_plaintext_database(),
    );
    crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
    let row = add(
        &db,
        "codex",
        "sql-original",
        "https://openrouter.ai/api/v1",
        "synthetic-secret",
    );
    let captured = capture(&db, &row);
    let (_, fence) = acquire_expected(db.clone(), &AppType::Codex, &captured).unwrap();
    let revision = crate::database::credential_revision::provider_on(
        &db.conn.lock().unwrap(),
        "codex",
        &row.provider_id,
    )
    .unwrap();
    let epoch = db.secrets.lifecycle_epoch();
    let snapshot = db.snapshot_to_memory().unwrap();
    let sql = crate::database::Database::dump_sql(&snapshot, &[]).unwrap();
    drop(snapshot);
    db.import_sql_string(&sql).unwrap();
    assert!(
        db.secrets.lifecycle_epoch() > epoch,
        "original SQL replacement must retire its runtime leases"
    );
    assert_eq!(
        crate::database::credential_revision::provider_on(
            &db.conn.lock().unwrap(),
            "codex",
            &row.provider_id
        )
        .unwrap(),
        revision,
        "restored row identity is not silently reissued by a read"
    );
    assert!(acquire_expected(db.clone(), &AppType::Codex, &captured).is_err());
    assert!(fence.check().is_err());
    acquire_expected(db.clone(), &AppType::Codex, &capture(&db, &row)).unwrap();
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify(case: &str) {
    match case {
        #[cfg(all(feature = "gui", feature = "test-hooks"))]
        "plaintext_sql_replacement" => plaintext_sql_replacement_retires_original_leases(),
        #[cfg(all(feature = "gui", feature = "test-hooks"))]
        "plaintext_label_preserves" | "plaintext_model_preserves" | "plaintext_recreate" => {
            plaintext_noncredential_or_recreate(case)
        }
        #[cfg(all(feature = "gui", feature = "test-hooks"))]
        "plaintext_request_reissue"
        | "plaintext_response_reissue"
        | "plaintext_request_aba"
        | "plaintext_response_aba" => plaintext_credential_retirement(case),
        "pricing_overrides" => credential_free_projection_keeps_original_pricing_overrides(),
        "request_options" => {
            acquisition_preserves_original_request_options_without_owner_or_destination_metadata()
        }
        "format_aliases" => protocol_aliases_follow_original_effective_precedence(),
        "projection" => projection_is_transient_and_preserves_original_wire_and_owner_id(),
        "expected_reseal" => {
            captured_request_rejects_same_key_reseal_while_new_request_remains_usable()
        }
        "fence_key" => acquired_fence_rejects_key_reissue(),
        "fence_epoch" => captured_and_acquired_leases_do_not_revive_after_session_unblock(),
        "unsupported" => {
            custom_endpoints_managed_bindings_and_unknown_formats_are_not_sources();
            #[cfg(feature = "gui")]
            default_claude_official_keeps_original_overview_and_sources_readable();
        }
        "ready" => original_rows_produce_opaque_credential_free_facts(),
        "destination" => exact_official_destination_is_required_not_brand_or_substring(),
        "reseal" => normal_reseal_retires_evidence_not_the_local_source_reference(),
        "invalid" => {
            missing_and_empty_are_not_sources_but_corruption_is_not_absence();
            malformed_endpoint_fields_and_original_credentials_remain_errors();
            captured_openrouter_endpoint_removal_retires_request_and_response();
        }
        _ => panic!("unknown synthetic OpenRouter owner fixture"),
    }
}
