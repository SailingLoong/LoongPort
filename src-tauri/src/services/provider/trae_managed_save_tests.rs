//! Original managed-save consumers, registered as a child of managed_inference_tests.
//! Synthetic opaque IDs stand for SG/US selections; no TRAE account or HTTP is created.
//! Safe model-name acceptance is a desired-behaviour RED on the current ASCII validator.
use super::*;

// Region is an owner fact, not encoded into or parsed from the Provider account ID.
const SG_ACCOUNT: &str = "d54da6b1-5242-4ad5-8a07-71581bcdca01";
const US_ACCOUNT: &str = "307fad77-2902-41e8-b65f-d858293b72b2";

fn trae_row(app: &AppType, id: &str, account_id: &str, model: &str) -> Provider {
    let mut provider = row(app, "trae");
    provider.id = id.into();
    let meta = provider.meta.as_mut().unwrap();
    // The original helper predates TRAE and only special-cases WorkBuddy.
    meta.api_format = Some("openai_chat".into());
    meta.auth_binding.as_mut().unwrap().account_id = Some(account_id.into());
    provider.settings_config = match app {
        AppType::Claude => serde_json::json!({"env": {
            "ANTHROPIC_MODEL": model,
            "ANTHROPIC_DEFAULT_SONNET_MODEL": model,
            "ANTHROPIC_DEFAULT_OPUS_MODEL": model,
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": model,
        }}),
        // This is the existing entry's JSON-string encoding inside a TOML value.
        AppType::Codex => serde_json::json!({
            "auth": {},
            "config": format!("model = {}\n", serde_json::to_string(model).unwrap()),
        }),
        _ => unreachable!(),
    };
    provider
}

fn native_files(fixture: &Fixture) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
    let mut paths = vec![fixture.path.clone()];
    if fixture.app == AppType::Codex {
        paths.push(crate::codex_config::get_codex_auth_path());
    }
    paths
        .into_iter()
        .map(|path| {
            assert!(path.starts_with(fixture._home.path()));
            let content = std::fs::read(&path).ok();
            (path, content)
        })
        .collect()
}

fn assert_projection(app: &AppType, provider: &Provider, account_id: &str, model: &str) {
    let meta = provider.meta.as_ref().unwrap();
    let binding = meta.auth_binding.as_ref().unwrap();
    assert!(matches!(binding.source, AuthBindingSource::ManagedAccount));
    assert_eq!(binding.auth_provider.as_deref(), Some("trae"));
    assert_eq!(binding.account_id.as_deref(), Some(account_id));
    assert_eq!(meta.api_format.as_deref(), Some("openai_chat"));
    assert_eq!(meta.common_config_enabled, Some(false));
    assert!(provider.uses_managed_account_auth());
    match app {
        AppType::Claude => {
            let settings = provider.settings_config.as_object().unwrap();
            assert_eq!(settings.len(), 1);
            let env = settings["env"].as_object().unwrap();
            assert_eq!(env.len(), 4);
            for key in [
                "ANTHROPIC_MODEL",
                "ANTHROPIC_DEFAULT_SONNET_MODEL",
                "ANTHROPIC_DEFAULT_OPUS_MODEL",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            ] {
                assert_eq!(env[key].as_str(), Some(model));
            }
        }
        AppType::Codex => {
            let settings = provider.settings_config.as_object().unwrap();
            assert_eq!(settings.len(), 2);
            assert!(settings["auth"].as_object().unwrap().is_empty());
            let text = settings["config"].as_str().unwrap();
            let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
            assert_eq!(doc.as_table().len(), 1);
            assert_eq!(doc["model"].as_str(), Some(model));
            // A quote in the model must remain one value, not another TOML key.
            let reparsed = doc.to_string().parse::<toml_edit::DocumentMut>().unwrap();
            assert_eq!(reparsed.as_table().len(), 1);
            assert_eq!(reparsed["model"].as_str(), Some(model));
        }
        _ => unreachable!(),
    }
}

fn assert_empty_route_saved_and_selected(app: AppType, account_id: &str, model: &str) {
    // Fixture::new/AppState::new are synchronous owners. Do not nest their runtime.
    let fixture = Fixture::new(app.clone(), true, false);
    // This fixture needs an initialized empty store, not an explicit empty user order.
    // The original migration legitimately materializes [] before any Provider exists.
    crate::proxy::application_routing::migrate(&fixture.state.db, app.as_str()).unwrap();
    assert_eq!(
        fixture
            .state
            .db
            .get_setting(&format!("application_priority_{}", app.as_str()))
            .unwrap()
            .as_deref(),
        Some("[]")
    );
    assert_eq!(
        crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str()).unwrap(),
        Vec::<String>::new()
    );
    let before = native_files(&fixture);
    let provider = trae_row(&app, "trae-safe-model", account_id, model);
    assert_projection(&app, &provider, account_id, model);
    let wire = serde_json::to_string(&provider).unwrap();
    let input: Provider = serde_json::from_str(&wire).unwrap();
    assert_eq!(input.settings_config, provider.settings_config);
    assert_projection(&app, &input, account_id, model);

    // Expected RED today: managed_inference::model rejects safe Unicode/space/quote.
    assert!(
        ProviderService::add(&fixture.state, app.clone(), input, false)
            .expect("safe model intent must reach the original save and application chain")
    );
    let saved = fixture
        .state
        .db
        .get_provider_by_id(&provider.id, app.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(saved.settings_config, provider.settings_config);
    assert_projection(&app, &saved, account_id, model);
    let decoded: Provider = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    assert_projection(&app, &decoded, account_id, model);
    assert_eq!(
        crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str()).unwrap(),
        vec![provider.id.clone()]
    );

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let routes = runtime.block_on(async {
        let mut config = fixture
            .state
            .db
            .get_proxy_config_for_app(app.as_str())
            .await
            .unwrap();
        config.auto_failover_enabled = true;
        fixture
            .state
            .db
            .update_proxy_config_for_app(config)
            .await
            .unwrap();
        let router = crate::proxy::provider_router::ProviderRouter::new(fixture.state.db.clone());
        let routes = router.select_providers(app.as_str()).await.unwrap();
        match app {
            AppType::Codex => assert_eq!(
                router.preferred_model(app.as_str(), &routes[0]).as_deref(),
                Some(model)
            ),
            AppType::Claude => {
                // Claude deliberately leaves preference unset; the real forwarder
                // resolves role/default model intent through this original mapper.
                assert_eq!(router.preferred_model(app.as_str(), &routes[0]), None);
                use crate::proxy::model_mapper::{
                    apply_model_mapping_detailed, strip_one_m_suffix_for_upstream_from_body,
                    ModelMappingKind,
                };
                for (requested, expected_kind) in [
                    ("claude-sonnet-synthetic", ModelMappingKind::Role),
                    ("claude-haiku-synthetic", ModelMappingKind::Role),
                    ("claude-opus-synthetic", ModelMappingKind::Role),
                    ("unmapped-client-model", ModelMappingKind::Default),
                ] {
                    let input = serde_json::json!({
                        "model": requested,
                        "messages": [{"role":"user", "content":"synthetic request"}],
                    });
                    let (mapped, original, selected, kind) =
                        apply_model_mapping_detailed(input.clone(), &routes[0]);
                    assert_eq!(original.as_deref(), Some(requested));
                    assert_eq!(selected.as_deref(), Some(model));
                    assert_eq!(kind, expected_kind);
                    let outgoing = strip_one_m_suffix_for_upstream_from_body(mapped);
                    assert_eq!(outgoing["model"].as_str(), Some(model));
                    assert_eq!(outgoing["messages"], input["messages"]);
                    let wire: serde_json::Value =
                        serde_json::from_str(&serde_json::to_string(&outgoing).unwrap()).unwrap();
                    assert_eq!(wire["model"].as_str(), Some(model));
                }
            }
            _ => unreachable!(),
        }
        routes
    });
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].id, provider.id);
    assert_projection(&app, &routes[0], account_id, model);
    assert_eq!(native_files(&fixture), before);
    assert_eq!(
        fixture.state.db.get_current_provider(app.as_str()).unwrap(),
        None
    );
    assert_eq!(fixture.mode().mode, Some(Mode::Direct));
}

#[test]
#[serial_test::serial]
fn trae_ascii_save_joins_initialized_empty_chain_without_native_activation() {
    for app in [AppType::Claude, AppType::Codex] {
        for account_id in [SG_ACCOUNT, US_ACCOUNT] {
            assert_empty_route_saved_and_selected(app.clone(), account_id, "synthetic-model");
        }
    }
}

#[test]
#[serial_test::serial]
fn trae_save_on_enabled_chain_is_routed_and_duplicate_id_cannot_replace_binding() {
    for app in [AppType::Claude, AppType::Codex] {
        let fixture = Fixture::new(app.clone(), true, true);
        let original_native = native_files(&fixture);
        crate::services::application_selection::apply_current_order_change(
            &fixture.state,
            &app,
            &["native".into()],
        )
        .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            fixture
                .state
                .proxy_service
                .set_takeover_for_app(app.as_str(), true)
                .await
                .unwrap();
            let mut config = fixture
                .state
                .db
                .get_proxy_config_for_app(app.as_str())
                .await
                .unwrap();
            config.auto_failover_enabled = true;
            fixture
                .state
                .db
                .update_proxy_config_for_app(config)
                .await
                .unwrap();
        });
        let before_save = native_files(&fixture);
        let sg = trae_row(&app, "trae-sg-route", SG_ACCOUNT, "synthetic-sg-model");
        let us = trae_row(&app, "trae-us-route", US_ACCOUNT, "synthetic-us-model");
        for provider in [&sg, &us] {
            assert!(
                ProviderService::add(&fixture.state, app.clone(), provider.clone(), false).unwrap()
            );
        }
        let expected_order = vec!["native", "trae-sg-route", "trae-us-route"];
        assert_eq!(
            crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str()).unwrap(),
            expected_order
        );
        let routes = runtime
            .block_on(
                crate::proxy::provider_router::ProviderRouter::new(fixture.state.db.clone())
                    .select_providers(app.as_str()),
            )
            .unwrap();
        assert_eq!(
            routes.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            expected_order
        );
        assert_projection(&app, &routes[1], SG_ACCOUNT, "synthetic-sg-model");
        assert_projection(&app, &routes[2], US_ACCOUNT, "synthetic-us-model");
        let saved_before = fixture
            .state
            .db
            .get_provider_by_id(&sg.id, app.as_str())
            .unwrap()
            .unwrap();
        assert!(
            ProviderService::add(&fixture.state, app.clone(), sg.clone(), false)
                .unwrap_err()
                .to_string()
                .contains("managedAccount.providerAlreadyExists")
        );
        let conflict = trae_row(&app, &sg.id, US_ACCOUNT, "different-model");
        assert!(
            ProviderService::add(&fixture.state, app.clone(), conflict, false)
                .unwrap_err()
                .to_string()
                .contains("managedAccount.providerAlreadyExists")
        );
        let saved_after = fixture
            .state
            .db
            .get_provider_by_id(&sg.id, app.as_str())
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(saved_after).unwrap(),
            serde_json::to_value(saved_before).unwrap()
        );
        assert_eq!(
            crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str()).unwrap(),
            expected_order
        );
        assert_eq!(native_files(&fixture), before_save);
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider(app.as_str())
                .unwrap()
                .as_deref(),
            Some("native")
        );
        runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app(app.as_str(), false),
            )
            .unwrap();
        assert_eq!(native_files(&fixture), original_native);
        assert_eq!(fixture.mode().mode, Some(Mode::Direct));
    }
}

#[test]
#[serial_test::serial]
fn trae_save_rejects_wrong_wire_format_before_any_row_or_native_change() {
    for app in [AppType::Claude, AppType::Codex] {
        let fixture = Fixture::new(app.clone(), true, true);
        crate::services::application_selection::apply_current_order_change(
            &fixture.state,
            &app,
            &["native".into()],
        )
        .unwrap();
        let before = native_files(&fixture);
        let mut provider = trae_row(&app, "trae-wrong-wire", SG_ACCOUNT, "synthetic-model");
        provider.meta.as_mut().unwrap().api_format = Some("anthropic".into());
        assert!(
            ProviderService::add(&fixture.state, app.clone(), provider.clone(), false)
                .unwrap_err()
                .to_string()
                .contains("managedAccount.invalidProjection")
        );
        assert!(fixture
            .state
            .db
            .get_provider_by_id(&provider.id, app.as_str())
            .unwrap()
            .is_none());
        assert_eq!(
            crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str()).unwrap(),
            vec!["native"]
        );
        assert_eq!(native_files(&fixture), before);
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider(app.as_str())
                .unwrap()
                .as_deref(),
            Some("native")
        );
    }
}

#[test]
#[serial_test::serial]
fn trae_sg_unicode_model_survives_claude_json_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Claude, SG_ACCOUNT, "模型-🧪");
}

#[test]
#[serial_test::serial]
fn trae_sg_unicode_model_survives_codex_toml_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Codex, SG_ACCOUNT, "模型-🧪");
}

#[test]
#[serial_test::serial]
fn trae_us_unicode_model_survives_claude_json_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Claude, US_ACCOUNT, "模型-🧪");
}

#[test]
#[serial_test::serial]
fn trae_us_unicode_model_survives_codex_toml_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Codex, US_ACCOUNT, "模型-🧪");
}

#[test]
#[serial_test::serial]
fn trae_sg_space_quote_model_survives_claude_json_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Claude, SG_ACCOUNT, "model family \"quoted\"");
}

#[test]
#[serial_test::serial]
fn trae_sg_space_quote_model_survives_codex_toml_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Codex, SG_ACCOUNT, "model family \"quoted\"");
}

#[test]
#[serial_test::serial]
fn trae_us_space_quote_model_survives_claude_json_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Claude, US_ACCOUNT, "model family \"quoted\"");
}

#[test]
#[serial_test::serial]
fn trae_us_space_quote_model_survives_codex_toml_save_and_router() {
    assert_empty_route_saved_and_selected(AppType::Codex, US_ACCOUNT, "model family \"quoted\"");
}

#[test]
#[serial_test::serial]
fn trae_invalid_model_boundaries_and_other_platform_unicode_never_save() {
    for app in [AppType::Claude, AppType::Codex] {
        let fixture = Fixture::new(app.clone(), true, true);
        crate::services::application_selection::apply_current_order_change(
            &fixture.state,
            &app,
            &["native".into()],
        )
        .unwrap();
        let before_native = native_files(&fixture);
        let before_rows =
            serde_json::to_value(fixture.state.db.get_all_providers(app.as_str()).unwrap())
                .unwrap();
        let invalid_trae_models = [
            String::new(),
            "   ".into(),
            "\u{2003}".into(),
            "model\nname".into(),
            "model\0name".into(),
            "model\u{0085}name".into(),
            "a".repeat(257),
            "界".repeat(86), // 86 characters, 258 UTF-8 bytes.
        ];
        for model in invalid_trae_models {
            let provider = trae_row(&app, "invalid-model-route", SG_ACCOUNT, &model);
            let error =
                ProviderService::add(&fixture.state, app.clone(), provider, false).unwrap_err();
            assert!(error
                .to_string()
                .contains("managedAccount.invalidProjection"));
        }
        for platform in ["workbuddy", "minimax_code", "zcode_start_plan"] {
            let mut provider = trae_row(&app, "invalid-model-route", US_ACCOUNT, "模型-🧪");
            let meta = provider.meta.as_mut().unwrap();
            meta.auth_binding.as_mut().unwrap().auth_provider = Some(platform.into());
            meta.api_format = Some(
                if platform == "workbuddy" {
                    "openai_chat"
                } else {
                    "anthropic"
                }
                .into(),
            );
            let error =
                ProviderService::add(&fixture.state, app.clone(), provider, false).unwrap_err();
            assert!(error
                .to_string()
                .contains("managedAccount.invalidProjection"));
        }
        assert_eq!(
            serde_json::to_value(fixture.state.db.get_all_providers(app.as_str()).unwrap())
                .unwrap(),
            before_rows
        );
        assert_eq!(
            crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str()).unwrap(),
            vec!["native"]
        );
        assert_eq!(native_files(&fixture), before_native);
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider(app.as_str())
                .unwrap()
                .as_deref(),
            Some("native")
        );
        assert_eq!(fixture.mode().mode, Some(Mode::Direct));
    }
}
