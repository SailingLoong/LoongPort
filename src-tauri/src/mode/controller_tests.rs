//! Actual service/controller fixtures. No live accounts or user directories.
use crate::app_config::AppType;
use crate::error::AppError;
use crate::live::engine::{read_current, DeviceStore};
use crate::mode::{
    operation::failpoint,
    state::{self, Mode},
};
use crate::provider::Provider;
use crate::secrets::testing::{initialize_database, TestHome};
use crate::services::ProviderService;
use crate::store::AppState;
use serde_json::json;
use std::sync::Arc;

struct Fixture {
    runtime: tokio::runtime::Runtime,
    state: AppState,
    path: std::path::PathBuf,
    _home: TestHome,
}
impl Fixture {
    fn claude() -> Self {
        Self::for_app(AppType::Claude)
    }
    fn for_app(app: AppType) -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(initialize_database().unwrap());
        crate::database::Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap())
            .unwrap();
        for id in ["a", "b"] {
            let config = match app {
                AppType::Claude => {
                    json!({"env": {"ANTHROPIC_BASE_URL": format!("https://synthetic-{id}.invalid"), "ANTHROPIC_AUTH_TOKEN": format!("synthetic-key-{id}"), "ANTHROPIC_MODEL": format!("synthetic-model-{id}")}})
                }
                AppType::Codex => {
                    json!({"auth": {"OPENAI_API_KEY": format!("synthetic-key-{id}")}, "config": format!("model = \"model-{id}\"\nmodel_provider = \"synthetic\"\n[model_providers.synthetic]\nname = \"Synthetic\"\nbase_url = \"https://{id}.example.invalid/v1\"\nwire_api = \"responses\"\n")})
                }
                AppType::Gemini => {
                    json!({"env": {"GEMINI_API_KEY": format!("synthetic-key-{id}"), "GOOGLE_GEMINI_BASE_URL": format!("https://{id}.example.invalid")}, "config": {"model": {"name": format!("model-{id}")}}})
                }
                AppType::GrokBuild => {
                    json!({"config": format!("[models]\ndefault = \"{id}\"\n[model.{id}]\nmodel = \"model-{id}\"\nname = \"synthetic-{id}\"\nbase_url = \"https://{id}.example.invalid/v1\"\napi_key = \"synthetic-key-{id}\"\napi_backend = \"responses\"\ncontext_window = 200000\n")})
                }
                _ => unreachable!(),
            };
            db.save_provider(
                app.as_str(),
                &Provider::with_id(id.into(), id.into(), config, None),
            )
            .unwrap();
        }
        db.set_current_provider(app.as_str(), "a").unwrap();
        crate::settings::set_current_provider(&app, Some("a")).unwrap();
        let state = AppState::new(db).unwrap();
        let path = match app {
            AppType::Claude => crate::config::get_claude_settings_path(),
            AppType::Codex => crate::codex_config::get_codex_config_path(),
            AppType::Gemini => crate::gemini_config::get_gemini_env_path(),
            AppType::GrokBuild => crate::grok_config::get_grok_config_path(),
            _ => unreachable!(),
        };
        if app == AppType::Claude {
            crate::config_file_io::write_durable(&path, br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-key-a","ANTHROPIC_BASE_URL":"https://synthetic-a.invalid","ANTHROPIC_MODEL":"synthetic-model-a"},"unowned":{"keep":true}}"#).unwrap();
        }
        state::update(
            &DeviceStore::for_device(),
            &state.db.secret_session().read().unwrap(),
            |live| {
                live.apps.entry(app.as_str().into()).or_default().mode = Some(Mode::Direct);
                Ok(())
            },
        )
        .unwrap();
        if app != AppType::Claude {
            ProviderService::switch(&state, app, "a").unwrap();
        }
        Self {
            _home: home,
            state,
            path,
            runtime: tokio::runtime::Runtime::new().unwrap(),
        }
    }
    fn pending(&self) -> Option<state::Pending> {
        state::pending(
            &DeviceStore::for_device(),
            &self.state.db.secret_session().read().unwrap(),
            "claude",
        )
        .unwrap()
    }
}
struct Fault;
impl Fault {
    fn at(point: &str) -> Self {
        failpoint::crash_at(Some(point));
        Self
    }
}
impl Drop for Fault {
    fn drop(&mut self) {
        failpoint::crash_at(None);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn existing_takeover_entry_records_intent_before_client_publication() {
    let fixture = Fixture::claude();
    let before = read_current(&fixture.path).unwrap();
    let _fault = Fault::at("pending");
    let result = fixture.runtime.block_on(
        fixture
            .state
            .proxy_service
            .set_takeover_for_app("claude", true),
    );
    assert!(
        result.is_err(),
        "existing takeover must use mode intent: {result:?}"
    );
    assert!(fixture.pending().is_some());
    assert_eq!(read_current(&fixture.path).unwrap(), before);
    assert!(
        crate::rt::block_on(fixture.state.db.get_live_backup("claude"))
            .unwrap()
            .is_none()
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn saving_current_row_keeps_database_old_until_its_file_intent_commits() {
    let fixture = Fixture::claude();
    let before = fixture
        .state
        .db
        .get_provider_by_id("a", "claude")
        .unwrap()
        .unwrap();
    let mut edited = before.clone();
    edited.name = "edited synthetic row".into();
    edited.settings_config["env"]["ANTHROPIC_MODEL"] = json!("changed-model");
    let _fault = Fault::at("pending");
    let result = ProviderService::update(&fixture.state, AppType::Claude, None, edited);
    assert!(
        result.is_err(),
        "current-row save must participate in the same intent: {result:?}"
    );
    assert!(fixture.pending().is_some());
    let current = fixture
        .state
        .db
        .get_provider_by_id("a", "claude")
        .unwrap()
        .unwrap();
    assert_eq!(current.name, before.name);
    assert_eq!(current.settings_config, before.settings_config);
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify() -> Result<(), AppError> {
    let enter =
        std::panic::catch_unwind(existing_takeover_entry_records_intent_before_client_publication);
    let save = std::panic::catch_unwind(
        saving_current_row_keeps_database_old_until_its_file_intent_commits,
    );
    let blocked =
        std::panic::catch_unwind(corrupt_blocked_inventory_refuses_takeover_without_effects);
    assert!(
        enter.is_ok() && save.is_ok() && blocked.is_ok(),
        "actual controller service entry and R3 intent"
    );
    println!("PASS existing proxy entry and current-row save share durable intent");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn original_proxy_server_binds_loopback_and_keeps_the_original_owner() {
    let fixture = Fixture::claude();
    let mut config = crate::rt::block_on(fixture.state.db.get_global_proxy_config()).unwrap();
    config.listen_address = "127.0.0.1".into();
    config.listen_port = 0;
    crate::rt::block_on(fixture.state.db.update_global_proxy_config(config)).unwrap();
    let service = fixture.state.proxy_service.clone();
    assert!(Arc::ptr_eq(&service, &service.owner().unwrap()));
    assert!(Arc::ptr_eq(service.database(), &fixture.state.db));
    assert!(Arc::ptr_eq(
        service.codex_manager(),
        &fixture.state.codex_oauth_manager
    ));
    crate::rt::block_on(async {
        let started = service.start().await.unwrap();
        assert_ne!(started.port, 0);
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://127.0.0.1:{}/health", started.port))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert!(service.is_running().await);
        service.stop().await.unwrap();
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", started.port))
            .await
            .is_err());
    });
    let weak = Arc::downgrade(&service);
    drop(service);
    drop(fixture);
    assert!(
        weak.upgrade().is_none(),
        "server ownership must not retain a strong service cycle"
    );
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_runtime() -> Result<(), AppError> {
    original_proxy_server_binds_loopback_and_keeps_the_original_owner();
    println!("PASS original ProxyServer loopback HTTP/start/stop and shared owner identity");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn corrupt_blocked_inventory_refuses_takeover_without_effects() {
    let fixture = Fixture::claude();
    fixture
        .state
        .db
        .set_setting("application_blocked_claude", "{bad-json")
        .unwrap();
    let before = read_current(&fixture.path).unwrap();
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let row = fixture
        .state
        .db
        .get_provider_by_id("a", "claude")
        .unwrap()
        .unwrap();
    let result = fixture.runtime.block_on(
        fixture
            .state
            .proxy_service
            .set_takeover_for_app("claude", true),
    );
    assert!(
        result.is_err(),
        "corrupt blocked facts cannot mean nobody is blocked"
    );
    assert_eq!(read_current(&fixture.path).unwrap(), before);
    assert_eq!(
        std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
        journal
    );
    assert_eq!(
        fixture
            .state
            .db
            .get_provider_by_id("a", "claude")
            .unwrap()
            .unwrap()
            .settings_config,
        row.settings_config
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn claude_enter_route_and_exit_keep_direct_selection_independent() {
    let fixture = Fixture::claude();
    fixture.runtime.block_on(async {
        fixture
            .state
            .proxy_service
            .set_takeover_for_app("claude", true)
            .await
            .unwrap();
        let mode = super::current::mode_state(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            &AppType::Claude,
        )
        .unwrap();
        assert!(mode.is_proxy() && mode.attached);
        fixture
            .state
            .proxy_service
            .switch_proxy_target("claude", "b")
            .await
            .unwrap();
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider("claude")
                .unwrap()
                .as_deref(),
            Some("a"),
            "route changes must not overwrite the direct pointer"
        );
        assert_eq!(
            super::current::provider_for(
                &fixture.state.db,
                &AppType::Claude,
                super::current::Purpose::InUse
            )
            .unwrap()
            .as_deref(),
            Some("b")
        );
        let routed = crate::proxy::provider_router::ProviderRouter::new(fixture.state.db.clone())
            .select_providers("claude")
            .await
            .unwrap();
        assert_eq!(
            routed[0].id, "b",
            "real request routing must use the committed proxy route"
        );
    });
    ProviderService::switch(&fixture.state, AppType::Claude, "a").unwrap();
    assert_eq!(
        super::current::provider_for(
            &fixture.state.db,
            &AppType::Claude,
            super::current::Purpose::InUse
        )
        .unwrap()
        .as_deref(),
        Some("a")
    );
    ProviderService::switch(&fixture.state, AppType::Claude, "b").unwrap();
    assert_eq!(
        fixture
            .state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("a")
    );
    fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("claude", false),
        )
        .unwrap();
    let live: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.path).unwrap()).unwrap();
    assert_eq!(live["env"]["ANTHROPIC_AUTH_TOKEN"], "synthetic-key-a");
    assert_eq!(live["unowned"]["keep"], true);
    assert!(fixture.pending().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn current_row_publication_and_target_failure_recover_as_one_operation() {
    for point in ["published:0", "target-db"] {
        let fixture = Fixture::claude();
        let before = fixture
            .state
            .db
            .get_provider_by_id("a", "claude")
            .unwrap()
            .unwrap();
        let mut edited = before.clone();
        edited.name = "edited-row".into();
        edited.settings_config["env"]["ANTHROPIC_MODEL"] = json!("changed-model");
        if point == "target-db" {
            fixture.state.db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_row BEFORE UPDATE ON providers WHEN NEW.name='edited-row' BEGIN SELECT RAISE(FAIL,'synthetic target failure'); END;").unwrap();
        }
        {
            let _fault = (point == "published:0").then(|| Fault::at(point));
            assert!(
                ProviderService::update(&fixture.state, AppType::Claude, None, edited.clone())
                    .is_err()
            );
        }
        assert_eq!(
            fixture
                .state
                .db
                .get_provider_by_id("a", "claude")
                .unwrap()
                .unwrap()
                .name,
            before.name
        );
        let expected: Provider = serde_json::from_value(
            fixture
                .pending()
                .unwrap()
                .target
                .saved_row
                .unwrap()
                .provider,
        )
        .unwrap();
        fixture
            .state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER IF EXISTS reject_row;")
            .unwrap();
        let _switch =
            futures::executor::block_on(fixture.state.proxy_service.lock_switch_for_app("claude"));
        assert_eq!(
            super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude)
                .unwrap(),
            Some(super::operation::RecoveryOutcome::RolledForward)
        );
        let saved = fixture
            .state
            .db
            .get_provider_by_id("a", "claude")
            .unwrap()
            .unwrap();
        assert_eq!(saved.name, edited.name);
        assert_eq!(saved.settings_config, expected.settings_config);
        let live: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.path).unwrap()).unwrap();
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "changed-model");
        assert!(fixture.pending().is_none());
    }
}

#[cfg_attr(test, test)]
fn row_commit_and_readback_finish_while_key_writer_waits() {
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let operation = std::thread::spawn(move || {
        let directory = crate::secrets::testing::tempdir().unwrap();
        let store = DeviceStore::at(directory.path());
        let db = Arc::new(crate::Database::memory().unwrap());
        let previous = Provider::with_id(
            "a".into(),
            "before".into(),
            json!({"env": {"secret":"synthetic-row-value"}}),
            None,
        );
        db.save_provider("claude", &previous).unwrap();
        let mut planned = previous.clone();
        planned.name = "after".into();
        let target = state::PendingTarget {
            saved_row: Some(state::SavedRow {
                before: crate::Database::provider_update_digest(&previous).unwrap(),
                provider: crate::Database::provider_update_value(&planned).unwrap(),
                clear_model_preference: false,
            }),
            ..Default::default()
        };
        let vault = db.secret_session().read().unwrap();
        state::update(&store, &vault, |live| {
            live.apps.entry("claude".into()).or_default().mode = Some(Mode::Direct);
            Ok(())
        })
        .unwrap();
        let session = db.secrets.clone();
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            waiting_tx.send(()).unwrap();
            let _key_writer = session.write().unwrap();
            acquired_tx.send(()).unwrap();
        });
        waiting_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert!(matches!(
            acquired_rx.recv_timeout(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        super::operation::commit_target(
            &db,
            db.secret_session(),
            &store,
            &vault,
            &AppType::Claude,
            &target,
        )
        .unwrap();
        assert_eq!(
            db.get_provider_by_id_with_vault("a", "claude", db.secret_session(), &vault)
                .unwrap()
                .unwrap()
                .name,
            "after"
        );
        let raw: String = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT settings_config FROM providers WHERE id='a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!raw.contains("synthetic-row-value"));
        drop(vault);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        writer.join().unwrap();
        finished_tx.send(()).unwrap();
    });
    finished_rx
        .recv_timeout(std::time::Duration::from_secs(4))
        .expect("held-vault row commit/readback must not take a recursive read lock");
    operation.join().unwrap();
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_recovery() -> Result<(), AppError> {
    let route =
        std::panic::catch_unwind(claude_enter_route_and_exit_keep_direct_selection_independent);
    let row = std::panic::catch_unwind(
        current_row_publication_and_target_failure_recover_as_one_operation,
    );
    let guard = std::panic::catch_unwind(row_commit_and_readback_finish_while_key_writer_waits);
    assert!(
        route.is_ok() && row.is_ok() && guard.is_ok(),
        "actual route/row recovery and pinned guard"
    );
    println!("PASS Claude route separation, R3 row/file recovery and waiting-key-writer commit");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn all_proxy_apps_enter_route_and_exit_through_existing_service() {
    let mut failed = Vec::new();
    for app in super::controller::PROXY_APPS {
        let result = std::panic::catch_unwind(|| {
            let fixture = Fixture::for_app(app.clone());
            fixture.runtime.block_on(async {
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app(app.as_str(), true)
                    .await
                    .unwrap();
                fixture
                    .state
                    .proxy_service
                    .switch_proxy_target(app.as_str(), "b")
                    .await
                    .unwrap();
                assert_eq!(
                    super::current::provider_for(
                        &fixture.state.db,
                        &app,
                        super::current::Purpose::InUse
                    )
                    .unwrap()
                    .as_deref(),
                    Some("b")
                );
                assert_eq!(
                    fixture
                        .state
                        .db
                        .get_current_provider(app.as_str())
                        .unwrap()
                        .as_deref(),
                    Some("a")
                );
                assert_eq!(
                    crate::proxy::provider_router::ProviderRouter::new(fixture.state.db.clone())
                        .select_providers(app.as_str())
                        .await
                        .unwrap()[0]
                        .id,
                    "b"
                );
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app(app.as_str(), false)
                    .await
                    .unwrap();
            });
            let vault = fixture.state.db.secret_session().read().unwrap();
            let mode =
                super::current::validate_known_mode(&DeviceStore::for_device(), &vault, &app)
                    .unwrap();
            assert_eq!(mode.mode, Some(Mode::Direct));
            assert!(!mode.attached);
            assert!(
                state::pending(&DeviceStore::for_device(), &vault, app.as_str())
                    .unwrap()
                    .is_none()
            );
        });
        if result.is_err() {
            failed.push(app.as_str().to_owned());
        } else {
            println!("PASS actual proxy enter/route/exit {}", app.as_str());
        }
    }
    assert!(failed.is_empty(), "proxy apps failed: {failed:?}");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn editing_proxy_route_and_direct_row_follow_distinct_applied_owners() {
    let fixture = Fixture::claude();
    fixture.runtime.block_on(async {
        fixture
            .state
            .proxy_service
            .set_takeover_for_app("claude", true)
            .await
            .unwrap();
        fixture
            .state
            .proxy_service
            .switch_proxy_target("claude", "b")
            .await
            .unwrap();
    });
    let mut route = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    route.settings_config["env"]["ANTHROPIC_MODEL"] = json!("edited-route-model");
    ProviderService::update(&fixture.state, AppType::Claude, None, route).unwrap();
    let proxy = std::fs::read(&fixture.path).unwrap();
    let live: serde_json::Value = serde_json::from_slice(&proxy).unwrap();
    assert_eq!(
        live["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL_NAME"],
        "edited-route-model"
    );
    assert_eq!(
        fixture
            .state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("a")
    );
    let mut direct = fixture
        .state
        .db
        .get_provider_by_id("a", "claude")
        .unwrap()
        .unwrap();
    direct.settings_config["env"]["ANTHROPIC_MODEL"] = json!("edited-direct-model");
    ProviderService::update(&fixture.state, AppType::Claude, None, direct).unwrap();
    assert_eq!(std::fs::read(&fixture.path).unwrap(), proxy);
    fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("claude", false),
        )
        .unwrap();
    let live: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.path).unwrap()).unwrap();
    assert_eq!(live["env"]["ANTHROPIC_MODEL"], "edited-direct-model");
    assert_eq!(live["unowned"]["keep"], true);
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_all_apps() -> Result<(), AppError> {
    editing_proxy_route_and_direct_row_follow_distinct_applied_owners();
    println!("PASS R3 applied proxy-route save and inactive direct save diverge safely");
    all_proxy_apps_enter_route_and_exit_through_existing_service();
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn service_stop_and_detach_use_mode_owner_before_stopping_listener() {
    let mut failed = Vec::new();
    for app in super::controller::PROXY_APPS {
        for keep in [false, true] {
            let result = std::panic::catch_unwind(|| {
                let mut fixture = Fixture::for_app(app.clone());
                fixture.runtime.block_on(async {
                    let mut config = fixture.state.db.get_global_proxy_config().await.unwrap();
                    config.listen_port = 0;
                    fixture
                        .state
                        .db
                        .update_global_proxy_config(config)
                        .await
                        .unwrap();
                    fixture
                        .state
                        .proxy_service
                        .set_takeover_for_app(app.as_str(), true)
                        .await
                        .unwrap();
                    fixture
                        .state
                        .proxy_service
                        .switch_proxy_target(app.as_str(), "b")
                        .await
                        .unwrap();
                    if keep {
                        fixture
                            .state
                            .proxy_service
                            .stop_with_restore_keep_state()
                            .await
                            .unwrap();
                    } else {
                        fixture
                            .state
                            .proxy_service
                            .stop_with_restore()
                            .await
                            .unwrap();
                    }
                });
                let mode = super::current::validate_known_mode(
                    &DeviceStore::for_device(),
                    &fixture.state.db.secret_session().read().unwrap(),
                    &app,
                )
                .unwrap();
                assert_eq!(
                    mode.mode,
                    Some(if keep { Mode::Proxy } else { Mode::Direct }),
                    "{app:?}/keep={keep}"
                );
                assert!(!mode.attached);
                assert_eq!(mode.proxy_route.as_deref(), Some("b"));
                assert_eq!(
                    fixture
                        .state
                        .db
                        .get_current_provider(app.as_str())
                        .unwrap()
                        .as_deref(),
                    Some("a")
                );
                assert!(!fixture
                    .runtime
                    .block_on(fixture.state.proxy_service.is_running()));
                assert!(fixture
                    .runtime
                    .block_on(fixture.state.db.get_live_backup(app.as_str()))
                    .unwrap()
                    .is_none());
                if keep {
                    let old = fixture
                        .runtime
                        .block_on(fixture.state.db.get_global_proxy_config())
                        .unwrap();
                    let held_old_port =
                        std::net::TcpListener::bind(("127.0.0.1", old.listen_port)).unwrap();
                    fixture.state = AppState::new(fixture.state.db.clone()).unwrap();
                    fixture.runtime.block_on(async {
                        let mut config = fixture.state.db.get_global_proxy_config().await.unwrap();
                        config.listen_port = 0;
                        fixture
                            .state
                            .db
                            .update_global_proxy_config(config)
                            .await
                            .unwrap();
                        fixture
                            .state
                            .proxy_service
                            .recover_from_crash()
                            .await
                            .unwrap();
                        assert!(fixture.state.proxy_service.is_running().await);
                        assert!(
                            super::current::validate_known_mode(
                                &DeviceStore::for_device(),
                                &fixture.state.db.secret_session().read().unwrap(),
                                &app
                            )
                            .unwrap()
                            .attached
                        );
                        let next = fixture.state.db.get_global_proxy_config().await.unwrap();
                        assert_ne!(next.listen_port, old.listen_port);
                        assert_eq!(
                            super::current::provider_for(
                                &fixture.state.db,
                                &app,
                                super::current::Purpose::InUse
                            )
                            .unwrap()
                            .as_deref(),
                            Some("b")
                        );
                        fixture
                            .state
                            .proxy_service
                            .stop_with_restore()
                            .await
                            .unwrap();
                    });
                    drop(held_old_port);
                }
            });
            if result.is_err() {
                failed.push(format!("{app:?}/keep={keep}"));
            }
        }
    }
    assert!(failed.is_empty(), "lifecycle failures: {failed:?}");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn failed_stop_retains_intent_and_listener_until_explicit_recovery() {
    for keep in [false, true] {
        let fixture = Fixture::claude();
        fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app("claude", true),
            )
            .unwrap();
        let result = {
            let _fault = Fault::at("published:0");
            fixture.runtime.block_on(async {
                if keep {
                    fixture
                        .state
                        .proxy_service
                        .stop_with_restore_keep_state()
                        .await
                } else {
                    fixture.state.proxy_service.stop_with_restore().await
                }
            })
        };
        assert!(result.is_err(), "the real stop must use mode publication");
        let pending = fixture
            .pending()
            .expect("the actual failed stop keeps its journal");
        let files = super::controller::files(&AppType::Claude)
            .unwrap()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>();
        assert!(
            fixture
                .runtime
                .block_on(fixture.state.proxy_service.is_running()),
            "listener cannot stop before live restoration is verified"
        );
        assert!(fixture
            .runtime
            .block_on(fixture.state.proxy_service.stop_with_restore())
            .is_err());
        assert_eq!(fixture.pending().unwrap(), pending);
        assert_eq!(
            files,
            super::controller::files(&AppType::Claude)
                .unwrap()
                .iter()
                .map(|f| read_current(&f.path).unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude)
                .unwrap(),
            Some(super::operation::RecoveryOutcome::RolledForward)
        );
        fixture
            .runtime
            .block_on(fixture.state.proxy_service.stop_with_restore())
            .unwrap();
        assert!(!fixture
            .runtime
            .block_on(fixture.state.proxy_service.is_running()));
        assert!(fixture.pending().is_none());
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_lifecycle() -> Result<(), AppError> {
    let switch_failure = std::panic::catch_unwind(
        profile_new_switch_failure_cannot_continue_into_mcp_or_mark_current,
    );
    let autosave =
        std::panic::catch_unwind(pending_profile_admission_precedes_old_profile_autosave);
    let missing_route =
        std::panic::catch_unwind(startup_proxy_without_saved_route_is_not_inferred_from_direct);
    let failover_enable =
        std::panic::catch_unwind(failed_failover_enable_retains_listener_for_its_pending_enter);
    assert!(
        switch_failure.is_ok()
            && autosave.is_ok()
            && missing_route.is_ok()
            && failover_enable.is_ok(),
        "review lifecycle regressions"
    );
    profile_stop_hint_respects_another_apps_pending_enter();
    queued_stop_rechecks_a_new_takeover_instead_of_using_old_hint();
    let mode =
        std::panic::catch_unwind(service_stop_and_detach_use_mode_owner_before_stopping_listener);
    let failed =
        std::panic::catch_unwind(failed_stop_retains_intent_and_listener_until_explicit_recovery);
    let startup = std::panic::catch_unwind(
        startup_keeps_pending_app_untouched_while_other_saved_route_attaches,
    );
    let sync =
        std::panic::catch_unwind(synchronous_disable_uses_same_intent_without_stopping_listener);
    let profile =
        std::panic::catch_unwind(profile_application_does_not_write_codex_mcp_through_pending);
    assert!(
        mode.is_ok() && failed.is_ok() && startup.is_ok() && sync.is_ok() && profile.is_ok(),
        "actual service lifecycle"
    );
    late_same_id_failover_cannot_reattach_direct_or_detached_app();
    lifecycle_missing_or_unknown_state_never_infers_from_legacy_flags();
    println!("PASS four-app stop/detach/new-port startup, failure isolation, sync/profile barriers and late same-ID failover");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn startup_keeps_pending_app_untouched_while_other_saved_route_attaches() {
    let mut fixture = Fixture::claude();
    let gemini = Provider::with_id(
        "g".into(),
        "synthetic-gemini".into(),
        json!({"env":{"GEMINI_API_KEY":"synthetic-key","GOOGLE_GEMINI_BASE_URL":"https://gemini.example.invalid"},"config":{"model":{"name":"synthetic-model"}}}),
        None,
    );
    fixture.state.db.save_provider("gemini", &gemini).unwrap();
    fixture
        .state
        .db
        .set_current_provider("gemini", "g")
        .unwrap();
    crate::settings::set_current_provider(&AppType::Gemini, Some("g")).unwrap();
    state::update(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        |live| {
            let entry = live.apps.entry("gemini".into()).or_default();
            entry.mode = Some(Mode::Proxy);
            entry.attached = false;
            entry.proxy_route = Some("g".into());
            Ok(())
        },
    )
    .unwrap();
    fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("claude", true),
        )
        .unwrap();
    {
        let _fault = Fault::at("published:0");
        assert!(fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .switch_proxy_target("claude", "b")
            )
            .is_err());
    }
    let pending = fixture.pending().unwrap();
    let bytes = read_current(&fixture.path).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop())
        .unwrap();
    fixture.state = AppState::new(fixture.state.db.clone()).unwrap();
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.recover_from_crash())
        .is_err());
    assert_eq!(fixture.pending().unwrap(), pending);
    assert_eq!(read_current(&fixture.path).unwrap(), bytes);
    let live = state::load(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
    )
    .unwrap();
    assert!(
        live.apps["gemini"].attached,
        "independent saved route can attach"
    );
    assert_eq!(live.apps["gemini"].proxy_route.as_deref(), Some("g"));
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.is_running()));
    assert!(fixture
        .runtime
        .block_on(fixture.state.db.get_live_backup("claude"))
        .unwrap()
        .is_none());
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.recover_from_crash())
        .unwrap();
    assert!(fixture.pending().is_none());
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn synchronous_disable_uses_same_intent_without_stopping_listener() {
    for app in super::controller::PROXY_APPS {
        let fixture = Fixture::for_app(app.clone());
        fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app(app.as_str(), true),
            )
            .unwrap();
        let files = super::controller::files(&app)
            .unwrap()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>();
        {
            let _fault = Fault::at("pending");
            assert!(fixture
                .state
                .proxy_service
                .disable_takeover_for_app_sync(&app)
                .is_err());
        }
        assert!(state::pending(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            app.as_str()
        )
        .unwrap()
        .is_some());
        assert_eq!(
            files,
            super::controller::files(&app)
                .unwrap()
                .iter()
                .map(|f| read_current(&f.path).unwrap())
                .collect::<Vec<_>>()
        );
        assert!(fixture
            .state
            .proxy_service
            .disable_takeover_for_app_sync(&app)
            .is_err());
        super::controller::recover_locked(&fixture.state.proxy_service, &app).unwrap();
        fixture
            .state
            .proxy_service
            .disable_takeover_for_app_sync(&app)
            .unwrap();
        assert_eq!(
            super::current::validate_known_mode(
                &DeviceStore::for_device(),
                &fixture.state.db.secret_session().read().unwrap(),
                &app
            )
            .unwrap()
            .mode,
            Some(Mode::Direct)
        );
        assert!(fixture
            .runtime
            .block_on(fixture.state.proxy_service.is_running()));
        fixture
            .runtime
            .block_on(fixture.state.proxy_service.stop_with_restore())
            .unwrap();
        fixture
            .state
            .proxy_service
            .disable_takeover_for_app_sync(&AppType::ClaudeDesktop)
            .unwrap();
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn profile_application_does_not_write_codex_mcp_through_pending() {
    let fixture = Fixture::for_app(AppType::Codex);
    let server = crate::app_config::McpServer {
        id: "synthetic-mcp".into(),
        name: "Synthetic".into(),
        server: json!({"command":"synthetic-command"}),
        apps: Default::default(),
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    };
    fixture.state.db.save_mcp_server(&server).unwrap();
    let profile = crate::database::Profile {
        id: "synthetic-profile".into(),
        name: "Synthetic".into(),
        payload: json!({"providers":{"codex":"a"},"mcp":{"codex":["synthetic-mcp"]}}).to_string(),
        sort_order: None,
        created_at: None,
        updated_at: None,
    };
    fixture.state.db.save_profile(&profile).unwrap();
    {
        let _fault = Fault::at("published:1");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err());
    }
    let files = super::controller::files(&AppType::Codex)
        .unwrap()
        .iter()
        .map(|f| read_current(&f.path).unwrap())
        .collect::<Vec<_>>();
    let pending = state::pending(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        "codex",
    )
    .unwrap()
    .unwrap();
    let result = crate::services::profile::ProfileService::apply(
        &fixture.state,
        "synthetic-profile",
        crate::services::profile::ProfileScope::Codex,
    );
    assert!(
        files
            == super::controller::files(&AppType::Codex)
                .unwrap()
                .iter()
                .map(|f| read_current(&f.path).unwrap())
                .collect::<Vec<_>>(),
        "profile must not pass disable failure into another writer: {result:?}"
    );
    assert!(result.is_err());
    assert_eq!(
        state::pending(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            "codex"
        )
        .unwrap()
        .unwrap(),
        pending
    );
    assert!(
        !fixture.state.db.get_all_mcp_servers().unwrap()["synthetic-mcp"]
            .apps
            .codex
    );
    assert!(fixture
        .state
        .db
        .get_current_profile_id("codex")
        .unwrap()
        .is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn late_same_id_failover_cannot_reattach_direct_or_detached_app() {
    for disposition in ["attached", "direct", "detached"] {
        let fixture = Fixture::claude();
        fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app("claude", true),
            )
            .unwrap();
        fixture
            .state
            .db
            .set_proxy_flags_sync("claude", true, true)
            .unwrap();
        let mut manager =
            crate::proxy::failover_switch::FailoverSwitchManager::new(fixture.state.db.clone());
        manager.set_service_owner(Arc::downgrade(&fixture.state.proxy_service));
        if disposition == "direct" {
            fixture
                .state
                .proxy_service
                .disable_takeover_for_app_sync(&AppType::Claude)
                .unwrap();
            // Model the stale compatibility flag window; it must not override mode.
            fixture
                .state
                .db
                .set_proxy_flags_sync("claude", true, true)
                .unwrap();
        } else if disposition == "detached" {
            fixture
                .runtime
                .block_on(fixture.state.proxy_service.stop_with_restore_keep_state())
                .unwrap();
            // A real listener may still serve an independent app. Running alone
            // must not reattach this app after the request started.
            fixture
                .runtime
                .block_on(fixture.state.proxy_service.start_for_mode())
                .unwrap();
        }
        assert!(fixture
            .runtime
            .block_on(fixture.state.proxy_service.is_running()));
        let before = read_current(&fixture.path).unwrap();
        let changed = fixture
            .runtime
            .block_on(manager.try_switch(
                #[cfg(feature = "gui")]
                None,
                "claude",
                "b",
                "synthetic-b",
                "a",
            ))
            .unwrap();
        assert_eq!(changed, disposition == "attached");
        if disposition != "attached" {
            assert_eq!(read_current(&fixture.path).unwrap(), before);
            assert_eq!(
                super::current::validate_known_mode(
                    &DeviceStore::for_device(),
                    &fixture.state.db.secret_session().read().unwrap(),
                    &AppType::Claude
                )
                .unwrap()
                .proxy_route
                .as_deref(),
                Some("a")
            );
        }
        assert_eq!(
            fixture
                .state
                .db
                .get_current_provider("claude")
                .unwrap()
                .as_deref(),
            Some("a")
        );
        assert!(fixture.pending().is_none());
        fixture
            .runtime
            .block_on(fixture.state.proxy_service.stop_with_restore())
            .unwrap();
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn lifecycle_missing_or_unknown_state_never_infers_from_legacy_flags() {
    for unknown in [false, true] {
        let fixture = Fixture::claude();
        fixture
            .state
            .db
            .set_proxy_flags_sync("claude", true, true)
            .unwrap();
        let store = DeviceStore::for_device();
        if unknown {
            let file = crate::secrets::owned_file::DeviceFile::registered(
                crate::secrets::owned_file::DEVICE_STATE_FILE,
            )
            .unwrap();
            let value = json!({"version":1,"future_mode_owner":true,"apps":{}});
            let bytes = file
                .encode(
                    &fixture.state.db.secret_session().read().unwrap(),
                    &serde_json::to_vec(&value).unwrap(),
                )
                .unwrap();
            std::fs::write(store.state_path(), bytes).unwrap();
        } else {
            std::fs::remove_file(store.state_path()).unwrap();
        }
        let state_before = read_current(&store.state_path()).unwrap();
        let bytes = read_current(&fixture.path).unwrap();
        assert!(fixture
            .runtime
            .block_on(fixture.state.proxy_service.recover_from_crash())
            .is_err());
        assert!(fixture
            .runtime
            .block_on(fixture.state.proxy_service.stop_with_restore())
            .is_err());
        assert_eq!(read_current(&fixture.path).unwrap(), bytes);
        assert_eq!(read_current(&store.state_path()).unwrap(), state_before);
        assert!(!fixture
            .runtime
            .block_on(fixture.state.proxy_service.is_running()));
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn profile_new_switch_failure_cannot_continue_into_mcp_or_mark_current() {
    let fixture = Fixture::for_app(AppType::Codex);
    fixture
        .state
        .db
        .save_mcp_server(&crate::app_config::McpServer {
            id: "synthetic-mcp".into(),
            name: "Synthetic".into(),
            server: json!({"command":"synthetic-command"}),
            apps: Default::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: vec![],
        })
        .unwrap();
    fixture
        .state
        .db
        .save_profile(&crate::database::Profile {
            id: "synthetic-profile".into(),
            name: "Synthetic".into(),
            payload: json!({"providers":{"codex":"b"},"mcp":{"codex":["synthetic-mcp"]}})
                .to_string(),
            sort_order: None,
            created_at: None,
            updated_at: None,
        })
        .unwrap();
    let result = {
        let _fault = Fault::at("published:1");
        crate::services::profile::ProfileService::apply(
            &fixture.state,
            "synthetic-profile",
            crate::services::profile::ProfileScope::Codex,
        )
    };
    let pending = state::pending(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        "codex",
    )
    .unwrap()
    .expect("actual profile provider switch creates intent");
    let file = pending
        .files
        .iter()
        .find(|f| f.path == fixture.path)
        .unwrap();
    assert_eq!(
        crate::live::engine::digest(read_current(&fixture.path).unwrap().as_deref()),
        file.planned,
        "MCP cannot overwrite the just-published file after switch failure"
    );
    assert!(result.is_err());
    assert!(
        !fixture.state.db.get_all_mcp_servers().unwrap()["synthetic-mcp"]
            .apps
            .codex
    );
    assert!(fixture
        .state
        .db
        .get_current_profile_id("codex")
        .unwrap()
        .is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn pending_profile_admission_precedes_old_profile_autosave() {
    let fixture = Fixture::for_app(AppType::Codex);
    for id in ["old", "next"] {
        fixture
            .state
            .db
            .save_profile(&crate::database::Profile {
                id: id.into(),
                name: id.into(),
                payload: "{}".into(),
                sort_order: None,
                created_at: None,
                updated_at: None,
            })
            .unwrap();
    }
    fixture
        .state
        .db
        .set_current_profile_id("codex", Some("old"))
        .unwrap();
    {
        let _fault = Fault::at("published:1");
        assert!(ProviderService::switch(&fixture.state, AppType::Codex, "b").is_err());
    }
    assert!(crate::services::profile::ProfileService::apply(
        &fixture.state,
        "next",
        crate::services::profile::ProfileScope::Codex
    )
    .is_err());
    let old = fixture.state.db.get_profile("old").unwrap().unwrap();
    assert_eq!(
        old.payload, "{}",
        "pending must be admitted before replacing the old profile snapshot"
    );
    assert_eq!(old.updated_at, None);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn startup_proxy_without_saved_route_is_not_inferred_from_direct() {
    let fixture = Fixture::claude();
    state::update(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        |live| {
            live.apps.get_mut("claude").unwrap().mode = Some(Mode::Proxy);
            Ok(())
        },
    )
    .unwrap();
    let before = read_current(&fixture.path).unwrap();
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.recover_from_crash())
        .is_err());
    assert_eq!(read_current(&fixture.path).unwrap(), before);
    assert!(!fixture
        .runtime
        .block_on(fixture.state.proxy_service.is_running()));
    assert!(fixture.pending().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn failed_failover_enable_retains_listener_for_its_pending_enter() {
    let fixture = Fixture::claude();
    {
        let _fault = Fault::at("published:0");
        assert!(fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_failover_for_app("claude", true)
            )
            .is_err());
    }
    assert!(fixture.pending().is_some());
    assert!(
        fixture
            .runtime
            .block_on(fixture.state.proxy_service.is_running()),
        "failed initial ENTER must not raw-stop based on still-false compatibility flags"
    );
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn profile_stop_hint_respects_another_apps_pending_enter() {
    let fixture = Fixture::claude();
    {
        let _fault = Fault::at("pending");
        assert!(fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app("claude", true)
            )
            .is_err());
    }
    fixture
        .state
        .db
        .save_profile(&crate::database::Profile {
            id: "desktop-profile".into(),
            name: "Synthetic".into(),
            payload: "{}".into(),
            sort_order: None,
            created_at: None,
            updated_at: None,
        })
        .unwrap();
    let (_, should_stop) = crate::services::profile::ProfileService::apply(
        &fixture.state,
        "desktop-profile",
        crate::services::profile::ProfileScope::ClaudeDesktop,
    )
    .unwrap();
    assert!(!should_stop, "compatibility flags are false before pending ENTER commits; they cannot authorize raw stop");
    assert!(fixture.pending().is_some());
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.is_running()));
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn queued_stop_rechecks_a_new_takeover_instead_of_using_old_hint() {
    let fixture = Fixture::claude();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.start_for_mode())
        .unwrap();
    assert!(!super::controller::needs_listener(&fixture.state.proxy_service).unwrap());
    // The old profile/UI hint was 'stop'. A new takeover wins before execution.
    fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("claude", true),
        )
        .unwrap();
    assert!(!fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_when_unused())
        .unwrap());
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.is_running()));
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_when_unused())
        .unwrap());
}
