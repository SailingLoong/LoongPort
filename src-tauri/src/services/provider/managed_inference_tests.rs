//! Real ProviderService / mode controller / router consumers, synthetic HOME only.
//! These tests require the original crate; the UI harness does not execute them.
use super::*;
use crate::live::engine::DeviceStore;
use crate::mode::{
    current,
    state::{self, Mode, PendingTarget},
};
use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
use crate::secrets::testing::TestHome;
use std::sync::Arc;

fn row(app: &AppType, auth_provider: &str) -> Provider {
    let mut provider = Provider::with_id(
        "account-route".into(),
        "Synthetic account".into(),
        match app {
            AppType::Claude => serde_json::json!({"env":{"ANTHROPIC_MODEL":"synthetic-model"}}),
            AppType::Codex => {
                serde_json::json!({"auth":{},"config":"model = \"synthetic-model\"\n"})
            }
            _ => unreachable!(),
        },
        None,
    );
    provider.category = Some("custom".into());
    provider.meta = Some(ProviderMeta {
        common_config_enabled: Some(false),
        api_format: Some(
            if auth_provider == "workbuddy" {
                "openai_chat"
            } else {
                "anthropic"
            }
            .into(),
        ),
        auth_binding: Some(AuthBinding {
            source: AuthBindingSource::ManagedAccount,
            auth_provider: Some(auth_provider.into()),
            account_id: Some("synthetic-account".into()),
        }),
        ..Default::default()
    });
    provider
}

struct Fixture {
    state: AppState,
    app: AppType,
    path: std::path::PathBuf,
    _home: TestHome,
}
impl Fixture {
    fn new(app: AppType, modern: bool, native: bool) -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(if modern {
            crate::secrets::testing::initialize_database().unwrap()
        } else {
            crate::secrets::testing::initialize_source17_database().unwrap()
        });
        if modern {
            crate::database::Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap())
                .unwrap();
        }
        // nextest runs tests in separate processes; each fixture needs its own
        // ephemeral listener instead of the shared default port.
        futures::executor::block_on(async {
            let mut config = db.get_global_proxy_config().await.unwrap();
            config.listen_address = "127.0.0.1".into();
            config.listen_port = 0;
            db.update_global_proxy_config(config).await.unwrap();
        });
        let state = AppState::new(db).unwrap();
        state::update(
            &DeviceStore::for_device(),
            &state.db.secret_session().read().unwrap(),
            |live| {
                live.apps.entry(app.as_str().into()).or_default().mode = Some(Mode::Direct);
                Ok(())
            },
        )
        .unwrap();
        let path = match app {
            AppType::Claude => crate::config::get_claude_settings_path(),
            AppType::Codex => crate::codex_config::get_codex_config_path(),
            _ => unreachable!(),
        };
        assert!(path.starts_with(home.path()));
        if native {
            let config = match app {
                AppType::Claude => {
                    serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-key","ANTHROPIC_BASE_URL":"https://native.invalid","ANTHROPIC_MODEL":"native-model"}})
                }
                AppType::Codex => {
                    serde_json::json!({"auth":{"OPENAI_API_KEY":"synthetic-key"},"config":"model = \"native-model\"\nmodel_provider = \"native\"\n[model_providers.native]\nname = \"Native fixture\"\nbase_url = \"https://native.invalid/v1\"\nwire_api = \"responses\"\n"})
                }
                _ => unreachable!(),
            };
            state
                .db
                .save_provider(
                    app.as_str(),
                    &Provider::with_id("native".into(), "Synthetic native".into(), config, None),
                )
                .unwrap();
            ProviderService::switch(&state, app.clone(), "native").unwrap();
        }
        Self {
            state,
            app,
            path,
            _home: home,
        }
    }
    fn mode(&self) -> state::ModeState {
        current::validate_known_mode(
            &DeviceStore::for_device(),
            &self.state.db.secret_session().read().unwrap(),
            &self.app,
        )
        .unwrap()
    }
}

#[test]
#[serial_test::serial]
fn first_account_add_uses_original_save_and_chain_without_native_write_or_current() {
    for modern in [false, true] {
        for app in [AppType::Claude, AppType::Codex] {
            for platform in ["workbuddy", "minimax_code", "zcode_start_plan"] {
                let fixture = Fixture::new(app.clone(), modern, false);
                let before = std::fs::read(&fixture.path).ok();
                assert!(ProviderService::add(
                    &fixture.state,
                    app.clone(),
                    row(&app, platform),
                    false
                )
                .unwrap());
                assert_eq!(std::fs::read(&fixture.path).ok(), before);
                assert_eq!(
                    fixture.state.db.get_current_provider(app.as_str()).unwrap(),
                    None
                );
                assert_eq!(
                    crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str())
                        .unwrap(),
                    vec!["account-route"]
                );
                assert_eq!(fixture.mode().mode, Some(Mode::Direct));
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn direct_selection_rejects_both_original_service_and_modern_application_controller() {
    for modern in [false, true] {
        for app in [AppType::Claude, AppType::Codex] {
            for platform in ["workbuddy", "minimax_code", "zcode_start_plan"] {
                let fixture = Fixture::new(app.clone(), modern, true);
                let provider = row(&app, platform);
                ProviderService::add(&fixture.state, app.clone(), provider.clone(), false).unwrap();
                let before = std::fs::read(&fixture.path).unwrap();
                assert!(
                    ProviderService::switch(&fixture.state, app.clone(), &provider.id)
                        .unwrap_err()
                        .to_string()
                        .contains("本地路由")
                );
                if modern {
                    let _guard = futures::executor::block_on(
                        fixture
                            .state
                            .proxy_service
                            .lock_switch_for_app(app.as_str()),
                    );
                    assert!(crate::mode::controller::select_locked_with_catalog(
                        &fixture.state.proxy_service,
                        &app,
                        &provider,
                        PendingTarget::default(),
                        None
                    )
                    .unwrap_err()
                    .to_string()
                    .contains("本地路由"));
                }
                assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
                assert_eq!(
                    fixture
                        .state
                        .db
                        .get_current_provider(app.as_str())
                        .unwrap()
                        .as_deref(),
                    Some("native")
                );
                assert!(state::pending(
                    &DeviceStore::for_device(),
                    &fixture.state.db.secret_session().read().unwrap(),
                    app.as_str()
                )
                .unwrap()
                .is_none());
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn entering_proxy_and_selecting_account_uses_original_mode_and_request_router() {
    for app in [AppType::Claude, AppType::Codex] {
        for platform in ["workbuddy", "minimax_code", "zcode_start_plan"] {
            let fixture = Fixture::new(app.clone(), true, true);
            let provider = row(&app, platform);
            ProviderService::add(&fixture.state, app.clone(), provider.clone(), false).unwrap();
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app(app.as_str(), true)
                    .await
                    .unwrap();
                assert!(fixture.mode().is_proxy());
                fixture
                    .state
                    .proxy_service
                    .switch_proxy_target(app.as_str(), &provider.id)
                    .await
                    .unwrap();
                let routes =
                    crate::proxy::provider_router::ProviderRouter::new(fixture.state.db.clone())
                        .select_providers(app.as_str())
                        .await
                        .unwrap();
                assert_eq!(routes[0].id, provider.id);
                assert_eq!(
                    routes[0]
                        .meta
                        .as_ref()
                        .unwrap()
                        .managed_account_id_for(platform)
                        .as_deref(),
                    Some("synthetic-account")
                );
                assert_eq!(
                    fixture
                        .state
                        .db
                        .get_current_provider(app.as_str())
                        .unwrap()
                        .as_deref(),
                    Some("native")
                );
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app(app.as_str(), false)
                    .await
                    .unwrap();
            });
            assert_eq!(fixture.mode().mode, Some(Mode::Direct));
        }
    }
}

#[test]
fn admission_and_presentation_keep_account_rows_out_of_key_editors() {
    for platform in ["workbuddy", "minimax_code", "zcode_start_plan"] {
        let provider = row(&AppType::Claude, platform);
        assert!(provider.uses_managed_account_auth());
        assert_eq!(
            provider_routing_requirement(&AppType::Claude, &provider),
            Some(ProviderRoutingReason::ManagedOAuth)
        );
        let context = ProviderPresentationContext {
            app_taken_over: false,
            current_provider_id: None,
            current_omo_provider_id: None,
            current_omo_slim_provider_id: None,
            configured_provider_ids: None,
            default_model_provider_id: None,
        };
        let view = provider_presentation_with_context(&context, &AppType::Claude, &provider);
        assert!(!view.can_edit && !view.can_test_connectivity && !view.can_configure_usage);
        assert!(view.can_delete);
    }
}

#[test]
fn managed_gateway_policy_requires_exact_platform_source_and_nonempty_identity() {
    for platform in ["workbuddy", "minimax_code", "zcode_start_plan"] {
        let mut provider = row(&AppType::Claude, platform);
        assert!(provider.uses_managed_account_auth());
        provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .account_id = Some("  ".into());
        assert!(!provider.uses_managed_account_auth());
        provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .account_id = Some("opaque".into());
        provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .source = AuthBindingSource::ProviderConfig;
        assert!(!provider.uses_managed_account_auth());
    }
    assert!(!row(&AppType::Claude, "not-a-managed-platform").uses_managed_account_auth());
}

#[test]
#[serial_test::serial]
fn initialized_enabled_chain_really_consumes_the_new_row_and_duplicate_add_cannot_overwrite() {
    for app in [AppType::Claude, AppType::Codex] {
        for platform in ["minimax_code", "zcode_start_plan"] {
            let fixture = Fixture::new(app.clone(), true, true);
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
            let before = std::fs::read(&fixture.path).unwrap();
            let provider = row(&app, platform);
            ProviderService::add(&fixture.state, app.clone(), provider.clone(), false).unwrap();
            assert_eq!(
                crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str())
                    .unwrap(),
                vec!["native", "account-route"]
            );
            runtime.block_on(async {
                let actual =
                    crate::proxy::provider_router::ProviderRouter::new(fixture.state.db.clone())
                        .select_providers(app.as_str())
                        .await
                        .unwrap();
                assert_eq!(
                    actual.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
                    vec!["native", "account-route"]
                );
            });
            assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
            assert!(
                ProviderService::add(&fixture.state, app.clone(), provider.clone(), false).is_err()
            );
            let mut conflict = provider.clone();
            conflict
                .meta
                .as_mut()
                .unwrap()
                .auth_binding
                .as_mut()
                .unwrap()
                .account_id = Some("different-account".into());
            assert!(ProviderService::add(&fixture.state, app.clone(), conflict, false).is_err());
            let actual = fixture
                .state
                .db
                .get_provider_by_id(&provider.id, app.as_str())
                .unwrap()
                .unwrap();
            assert_eq!(
                actual
                    .meta
                    .unwrap()
                    .managed_account_id_for(platform)
                    .as_deref(),
                Some("synthetic-account")
            );
            assert_eq!(
                crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str())
                    .unwrap(),
                vec!["native", "account-route"]
            );
            runtime
                .block_on(
                    fixture
                        .state
                        .proxy_service
                        .set_takeover_for_app(app.as_str(), false),
                )
                .unwrap();
        }
    }
}

#[test]
#[serial_test::serial]
fn model_only_account_entry_rejects_original_request_override_metadata_before_save() {
    let fixture = Fixture::new(AppType::Codex, true, false);
    let mut provider = row(&AppType::Codex, "zcode_start_plan");
    provider
        .meta
        .as_mut()
        .unwrap()
        .local_proxy_request_overrides = Some(crate::provider::LocalProxyRequestOverrides {
        body: Some(serde_json::json!({"model":"different-model"})),
        ..Default::default()
    });
    assert!(ProviderService::add(&fixture.state, AppType::Codex, provider.clone(), false).is_err());
    assert!(fixture
        .state
        .db
        .get_provider_by_id(&provider.id, "codex")
        .unwrap()
        .is_none());
}

#[test]
#[serial_test::serial]
fn original_source_bound_model_update_is_preserved_but_not_accepted_as_new_entry() {
    let fixture = Fixture::new(AppType::Codex, true, false);
    let provider = row(&AppType::Codex, "zcode_start_plan");
    ProviderService::add(&fixture.state, AppType::Codex, provider.clone(), false).unwrap();
    let patched = super::model_settings::apply_patch(
        &AppType::Codex,
        &provider,
        &super::model_settings::ModelSettingsPatch {
            model: Some(super::model_settings::FieldChange::Set(
                "new-synthetic-model".into(),
            )),
            ..Default::default()
        },
    )
    .unwrap();
    super::model_settings::validate_change(&AppType::Codex, &provider, &patched).unwrap();
    assert!(super::model_settings::owned_scope_matches(&patched));
    let prepared =
        ProviderService::prepare_provider_update(&fixture.state, &AppType::Codex, patched.clone())
            .unwrap();
    assert_eq!(prepared.settings_config, patched.settings_config);
    assert!(prepared.meta.unwrap().application_model_overrides.is_some());
    let error = ProviderService::add(&fixture.state, AppType::Codex, patched, false).unwrap_err();
    assert!(error
        .to_string()
        .contains("managedAccount.invalidProjection"));
}

#[path = "trae_managed_save_tests.rs"]
mod trae_managed_save_tests;
