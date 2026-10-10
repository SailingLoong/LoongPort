//! Actual service/controller fixtures. No live accounts or user directories.
use crate::app_config::AppType;
#[cfg(feature = "test-hooks")]
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
        Self::for_app_with_schema(app, true)
    }
    fn for_app_with_schema(app: AppType, modern: bool) -> Self {
        let home = TestHome::new().unwrap();
        crate::settings::reload_settings().unwrap();
        let db = Arc::new(initialize_database().unwrap());
        let mut settings = crate::settings::get_settings();
        settings.claude_config_dir =
            Some(home.path().join(".claude").to_string_lossy().into_owned());
        crate::settings::update_settings(settings).unwrap();
        if modern {
            crate::database::Database::apply_upstream4_migrations_on_conn(&db.conn.lock().unwrap())
                .unwrap();
        }
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

fn lifecycle_insert_future_peer(fixture: &Fixture) -> String {
    let store = DeviceStore::for_device();
    let vault = fixture.state.db.secret_session().read().unwrap();
    let file = crate::secrets::owned_file::DeviceFile::registered(
        crate::secrets::owned_file::DEVICE_STATE_FILE,
    )
    .unwrap();
    let plain = store.read_device(&vault, &file).unwrap().unwrap();
    let mut fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(&plain).unwrap();
    let mut apps: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_str(fields["apps"].get()).unwrap();
    let peer = r#"{"mode":"future-mode","pending":{"future":true},"opaque":900719925474099312345}"#;
    apps.insert("codex".into(), serde_json::from_str(peer).unwrap());
    fields.insert(
        "apps".into(),
        serde_json::value::RawValue::from_string(serde_json::to_string(&apps).unwrap()).unwrap(),
    );
    store
        .write_device(&vault, &file, &serde_json::to_vec(&fields).unwrap())
        .unwrap();
    peer.into()
}

fn lifecycle_read_future_peer(fixture: &Fixture) -> String {
    let store = DeviceStore::for_device();
    let vault = fixture.state.db.secret_session().read().unwrap();
    let file = crate::secrets::owned_file::DeviceFile::registered(
        crate::secrets::owned_file::DEVICE_STATE_FILE,
    )
    .unwrap();
    let plain = store.read_device(&vault, &file).unwrap().unwrap();
    let fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(&plain).unwrap();
    let apps: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_str(fields["apps"].get()).unwrap();
    apps["codex"].get().into()
}

#[test]
#[serial_test::serial]
fn u03_lifecycle_restore_proven_app_despite_future_peer() {
    let fixture = Fixture::claude();
    ProviderService::switch(&fixture.state, AppType::Claude, "a").unwrap();
    let native = read_current(&fixture.path).unwrap();
    fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("claude", true),
        )
        .unwrap();
    assert_ne!(read_current(&fixture.path).unwrap(), native);
    let peer = lifecycle_insert_future_peer(&fixture);
    let error = fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore_keep_state())
        .unwrap_err();
    assert_eq!(
        read_current(&fixture.path).unwrap(),
        native,
        "future peer must not block this app's original native restoration"
    );
    assert!(
        error.contains("codex"),
        "partial result names the refused app: {error}"
    );
    let mode = state::mode_state(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        "claude",
    )
    .unwrap();
    assert_eq!(mode.mode, Some(Mode::Proxy));
    assert!(!mode.attached);
    assert_eq!(mode.proxy_route.as_deref(), Some("a"));
    assert!(fixture.pending().is_none());
    assert_eq!(
        fixture
            .state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("a")
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Claude)
            .unwrap()
            .as_deref(),
        Some("a")
    );
    assert_eq!(lifecycle_read_future_peer(&fixture), peer);
    assert!(super::controller::needs_listener(&fixture.state.proxy_service).unwrap());
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.is_running()));
}

#[test]
#[serial_test::serial]
fn u03_lifecycle_startup_proven_app_despite_future_peer() {
    let fixture = Fixture::claude();
    ProviderService::switch(&fixture.state, AppType::Claude, "a").unwrap();
    let native = read_current(&fixture.path).unwrap();
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
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore_keep_state())
        .unwrap();
    assert_eq!(read_current(&fixture.path).unwrap(), native);
    let peer = lifecycle_insert_future_peer(&fixture);
    let error = fixture
        .runtime
        .block_on(fixture.state.proxy_service.recover_from_crash())
        .unwrap_err();
    let mode = state::mode_state(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        "claude",
    )
    .unwrap();
    assert!(
        mode.attached,
        "future peer must not block this app's original startup projection"
    );
    assert_eq!(mode.mode, Some(Mode::Proxy));
    assert_eq!(mode.proxy_route.as_deref(), Some("a"));
    assert!(
        error.contains("codex"),
        "partial result names the refused app: {error}"
    );
    let projected: serde_json::Value =
        serde_json::from_slice(&read_current(&fixture.path).unwrap().unwrap()).unwrap();
    let (base, _) = fixture
        .runtime
        .block_on(fixture.state.proxy_service.build_proxy_urls())
        .unwrap();
    assert_eq!(projected["env"]["ANTHROPIC_BASE_URL"], base);
    assert_eq!(projected["unowned"], json!({"keep":true}));
    assert!(fixture.pending().is_none());
    assert_eq!(lifecycle_read_future_peer(&fixture), peer);
    assert!(fixture
        .runtime
        .block_on(fixture.state.proxy_service.is_running()));
}

#[test]
#[serial_test::serial]
fn u03_lifecycle_future_peer_conservatively_retains_listener() {
    let fixture = Fixture::claude();
    let peer = lifecycle_insert_future_peer(&fixture);
    assert!(
        super::controller::needs_listener(&fixture.state.proxy_service).unwrap(),
        "opaque peer is not proof that the listener can stop"
    );
    assert_eq!(lifecycle_read_future_peer(&fixture), peer);
    let store = DeviceStore::for_device();
    let vault = fixture.state.db.secret_session().read().unwrap();
    let file = crate::secrets::owned_file::DeviceFile::registered(
        crate::secrets::owned_file::DEVICE_STATE_FILE,
    )
    .unwrap();
    for shared in [
        br#"{"version":1,"future_root":true,"apps":{}}"#.as_slice(),
        br#"{"version":99,"apps":{}}"#.as_slice(),
    ] {
        store.write_device(&vault, &file, shared).unwrap();
        let before = read_current(&store.state_path()).unwrap();
        assert!(super::controller::needs_listener(&fixture.state.proxy_service).is_err());
        assert_eq!(read_current(&store.state_path()).unwrap(), before);
    }
    std::fs::write(store.state_path(), b"synthetic-invalid-ciphertext").unwrap();
    assert!(super::controller::needs_listener(&fixture.state.proxy_service).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn app_scoped_original_recovery_restores_settings_and_db_without_touching_future_peer() {
    let fixture = Fixture::claude();
    let store = DeviceStore::for_device();
    let vault = fixture.state.db.secret_session().read().unwrap();
    let file = crate::secrets::owned_file::DeviceFile::registered(
        crate::secrets::owned_file::DEVICE_STATE_FILE,
    )
    .unwrap();
    let mut value = serde_json::to_value(state::load(&store, &vault).unwrap()).unwrap();
    let peer = json!({"mode":"future-mode","opaque":{"synthetic":"keep"}});
    value["apps"]["gemini"] = peer.clone();
    value["apps"]["claude"]["pending"] = serde_json::to_value(state::Pending {
        op: state::op::SWITCH.into(),
        files: vec![],
        target: state::PendingTarget::pointer(Some("b".into())),
        published: true,
        extra: Default::default(),
    })
    .unwrap();
    store
        .write_device(&vault, &file, &serde_json::to_vec(&value).unwrap())
        .unwrap();
    drop(vault);
    let before_client = read_current(&fixture.path).unwrap();
    fixture.state.db.conn.lock().unwrap().execute_batch("CREATE TRIGGER synthetic_recovery_fault BEFORE UPDATE OF is_current ON providers WHEN NEW.id='b' AND NEW.is_current=1 BEGIN SELECT RAISE(ABORT,'synthetic recovery interruption'); END;").unwrap();
    assert!(
        super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).is_err()
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Claude)
            .unwrap()
            .as_deref(),
        Some("b")
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
    assert!(fixture.pending().unwrap().published);
    fixture
        .state
        .db
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER synthetic_recovery_fault;")
        .unwrap();
    assert_eq!(
        super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap(),
        Some(super::operation::RecoveryOutcome::RolledForward)
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Claude)
            .unwrap()
            .as_deref(),
        Some("b")
    );
    assert_eq!(
        fixture
            .state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("b")
    );
    let vault = fixture.state.db.secret_session().read().unwrap();
    assert_eq!(
        state::mode_state(&store, &vault, "claude").unwrap().mode,
        Some(Mode::Direct)
    );
    assert!(state::pending(&store, &vault, "claude").unwrap().is_none());
    assert!(state::mode_state(&store, &vault, "gemini").is_err());
    let plain = store.read_device(&vault, &file).unwrap().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&plain).unwrap()["apps"]["gemini"],
        peer
    );
    assert_eq!(read_current(&fixture.path).unwrap(), before_client);
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
        let mut manager = fixture.runtime.block_on(
            fixture
                .state
                .proxy_service
                .active_failover_manager_for_test(),
        );
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
            // Exercise the current listener with a detached app; old-listener
            // rejection has its separate real HTTP restart regression.
            manager = fixture.runtime.block_on(
                fixture
                    .state
                    .proxy_service
                    .active_failover_manager_for_test(),
            );
        }
        assert!(fixture
            .runtime
            .block_on(fixture.state.proxy_service.is_running()));
        let before = read_current(&fixture.path).unwrap();
        let identity = fixture
            .state
            .proxy_service
            .request_identity("claude")
            .unwrap();
        let changed = fixture
            .runtime
            .block_on(manager.try_switch(
                #[cfg(feature = "gui")]
                None,
                "claude",
                "b",
                "synthetic-b",
                "a",
                Some(&identity),
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

fn selection_fixture() -> Fixture {
    let fixture = Fixture::claude();
    let mut row = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    row.settings_config["modelCatalog"] =
        json!({"models":[{"model":"synthetic-model-b"},{"model":"selected-model"}]});
    fixture.state.db.save_provider("claude", &row).unwrap();
    fixture
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn selected_model_and_row_wait_for_the_same_live_intent() {
    let fixture = selection_fixture();
    let before = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    let files = read_current(&fixture.path).unwrap();
    let result = {
        let _fault = Fault::at("pending");
        crate::services::application_selection::select_with_commit(
            &fixture.state,
            &AppType::Claude,
            &crate::services::application_selection::TierSelection {
                provider_id: "b".into(),
                model: Some("selected-model".into()),
            },
            None,
        )
    };
    assert!(
        result.is_err(),
        "actual selection must enter the controlled operation: {result:?}"
    );
    assert!(fixture.pending().is_some());
    assert_eq!(
        fixture
            .state
            .db
            .get_provider_by_id("b", "claude")
            .unwrap()
            .unwrap()
            .settings_config,
        before.settings_config
    );
    assert!(crate::proxy::auto_strategy::get_model_pref(&fixture.state.db, "claude").is_none());
    assert_eq!(read_current(&fixture.path).unwrap(), files);
    assert_eq!(
        fixture
            .state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("a")
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn selected_model_refuses_existing_pending_without_legacy_write() {
    let fixture = selection_fixture();
    {
        let _fault = Fault::at("published:0");
        assert!(ProviderService::switch(&fixture.state, AppType::Claude, "b").is_err());
    }
    let pending = fixture.pending().unwrap();
    let files = read_current(&fixture.path).unwrap();
    let before = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    let result = crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Claude,
        &crate::services::application_selection::TierSelection {
            provider_id: "b".into(),
            model: Some("selected-model".into()),
        },
        None,
    );
    assert!(
        result.is_err(),
        "pending must block the actual selection service"
    );
    assert_eq!(fixture.pending().unwrap(), pending);
    assert_eq!(read_current(&fixture.path).unwrap(), files);
    assert_eq!(
        fixture
            .state
            .db
            .get_provider_by_id("b", "claude")
            .unwrap()
            .unwrap()
            .settings_config,
        before.settings_config
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn proxy_selection_does_not_restore_old_snapshot_behind_pending() {
    let fixture = selection_fixture();
    fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("claude", true),
        )
        .unwrap();
    let before = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    let result = {
        let _fault = Fault::at("published:0");
        crate::services::application_selection::select_with_commit(
            &fixture.state,
            &AppType::Claude,
            &crate::services::application_selection::TierSelection {
                provider_id: "b".into(),
                model: Some("selected-model".into()),
            },
            None,
        )
    };
    assert!(result.is_err());
    let pending = fixture.pending().unwrap();
    assert_eq!(
        crate::live::engine::digest(read_current(&fixture.path).unwrap().as_deref()),
        pending.files[0].planned,
        "legacy snapshot rollback must not overwrite the published pending image"
    );
    assert_eq!(
        fixture
            .state
            .db
            .get_provider_by_id("b", "claude")
            .unwrap()
            .unwrap()
            .settings_config,
        before.settings_config
    );
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_selection() -> Result<(), AppError> {
    plugin_post_selection_follows_attached_route_and_detached_direct_owner();
    let normalize =
        std::panic::catch_unwind(selected_model_survives_legacy_common_snippet_normalization);
    let plugin =
        std::panic::catch_unwind(modern_selection_preserves_claude_plugin_post_switch_hook);
    let order_entry =
        std::panic::catch_unwind(bare_legacy_order_entry_cannot_write_through_modern_pending);
    assert!(
        normalize.is_ok() && plugin.is_ok() && order_entry.is_ok(),
        "R3 reviewed entry regressions"
    );
    let intent = std::panic::catch_unwind(selected_model_and_row_wait_for_the_same_live_intent);
    let pending =
        std::panic::catch_unwind(selected_model_refuses_existing_pending_without_legacy_write);
    let proxy =
        std::panic::catch_unwind(proxy_selection_does_not_restore_old_snapshot_behind_pending);
    assert!(
        intent.is_ok() && pending.is_ok() && proxy.is_ok(),
        "actual R3 selection journals"
    );
    combined_selection_restarts_with_row_model_order_and_mode_together();
    order_only_changes_use_zero_file_intent_and_recovery_for_all_apps();
    changed_order_blocks_replay_before_remaining_codex_files();
    actual_model_or_order_sql_failure_retains_combined_target_until_recovery();
    explicit_model_actions_round_trip_without_null_collapse();
    current_priority_service_retains_named_owner_and_refuses_pending();
    r3_clear_model_action_survives_actual_restart_and_recovery();
    r3_selection_reports_preserved_external_codex_catalog();
    r3_target_readback_finishes_with_a_waiting_vault_writer();
    println!("PASS R3 actual selection, typed model/order recovery, conflicts and SQL failures");
    Ok(())
}

fn routing_settings(db: &crate::database::Database, app: &AppType) -> Vec<Option<String>> {
    [
        format!("application_order_profiles_{}", app.as_str()),
        format!("application_order_profile_current_{}", app.as_str()),
        format!("application_priority_{}", app.as_str()),
    ]
    .iter()
    .map(|key| db.get_setting(key).unwrap())
    .collect()
}
fn selection_fixture_for(app: AppType) -> Fixture {
    let fixture = Fixture::for_app(app.clone());
    let mut row = fixture
        .state
        .db
        .get_provider_by_id("b", app.as_str())
        .unwrap()
        .unwrap();
    row.settings_config["modelCatalog"] = json!({"models":[{"model":"selected-model"}]});
    fixture.state.db.save_provider(app.as_str(), &row).unwrap();
    fixture
}
fn selected_model() -> crate::services::application_selection::TierSelection {
    crate::services::application_selection::TierSelection {
        provider_id: "b".into(),
        model: Some("selected-model".into()),
    }
}
fn selected_order() -> crate::services::application_selection::RoutingOrder {
    crate::services::application_selection::RoutingOrder {
        profile_name: "default".into(),
        provider_ids: vec!["b".into(), "a".into()],
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn combined_selection_restarts_with_row_model_order_and_mode_together() {
    for app in super::controller::PROXY_APPS {
        for disposition in ["direct", "proxy", "detached"] {
            for stage in if disposition == "detached" {
                vec!["pending", "target"]
            } else {
                vec!["pending", "publication", "target"]
            } {
                let mut fixture = selection_fixture_for(app.clone());
                if disposition != "direct" {
                    fixture
                        .runtime
                        .block_on(
                            fixture
                                .state
                                .proxy_service
                                .set_takeover_for_app(app.as_str(), true),
                        )
                        .unwrap();
                }
                if disposition == "detached" {
                    fixture
                        .runtime
                        .block_on(fixture.state.proxy_service.stop_with_restore_keep_state())
                        .unwrap();
                }
                let before = fixture
                    .state
                    .db
                    .get_provider_by_id("b", app.as_str())
                    .unwrap()
                    .unwrap();
                let order_before = routing_settings(&fixture.state.db, &app);
                let model_before = crate::proxy::auto_strategy::get_model_pref_checked(
                    &fixture.state.db,
                    app.as_str(),
                )
                .unwrap();
                let files_before = super::controller::files(&app)
                    .unwrap()
                    .iter()
                    .map(|f| read_current(&f.path).unwrap())
                    .collect::<Vec<_>>();
                let point = if stage == "publication" {
                    if app == AppType::Codex || app == AppType::Gemini {
                        "published:1"
                    } else {
                        "published:0"
                    }
                } else {
                    stage
                };
                let result = {
                    let _fault = Fault::at(point);
                    crate::services::application_selection::select_with_commit(
                        &fixture.state,
                        &app,
                        &selected_model(),
                        Some(&selected_order()),
                    )
                };
                assert!(result.is_err(), "{app:?}/{disposition}/{point}: {result:?}");
                let pending = state::pending(
                    &DeviceStore::for_device(),
                    &fixture.state.db.secret_session().read().unwrap(),
                    app.as_str(),
                )
                .unwrap()
                .expect("actual intent");
                let intended =
                    super::operation::saved_provider(pending.target.saved_row.as_ref().unwrap())
                        .unwrap();
                if stage == "pending" {
                    assert_eq!(
                        crate::database::Database::provider_update_digest(
                            &fixture
                                .state
                                .db
                                .get_provider_by_id("b", app.as_str())
                                .unwrap()
                                .unwrap()
                        )
                        .unwrap(),
                        crate::database::Database::provider_update_digest(&before).unwrap()
                    );
                    assert_eq!(routing_settings(&fixture.state.db, &app), order_before);
                    assert_eq!(
                        crate::proxy::auto_strategy::get_model_pref_checked(
                            &fixture.state.db,
                            app.as_str()
                        )
                        .unwrap(),
                        model_before
                    );
                    assert_eq!(
                        files_before,
                        super::controller::files(&app)
                            .unwrap()
                            .iter()
                            .map(|f| read_current(&f.path).unwrap())
                            .collect::<Vec<_>>()
                    );
                }
                if fixture
                    .runtime
                    .block_on(fixture.state.proxy_service.is_running())
                {
                    fixture
                        .runtime
                        .block_on(fixture.state.proxy_service.stop())
                        .unwrap();
                }
                fixture.state = AppState::new(fixture.state.db.clone()).unwrap();
                let recovered =
                    super::controller::recover_locked(&fixture.state.proxy_service, &app).unwrap();
                assert_eq!(
                    recovered,
                    Some(if stage == "pending" {
                        super::operation::RecoveryOutcome::Discarded
                    } else {
                        super::operation::RecoveryOutcome::RolledForward
                    }),
                    "{app:?}/{disposition}/{point}"
                );
                assert!(state::pending(
                    &DeviceStore::for_device(),
                    &fixture.state.db.secret_session().read().unwrap(),
                    app.as_str()
                )
                .unwrap()
                .is_none());
                if stage != "pending" {
                    assert_eq!(
                        fixture
                            .state
                            .db
                            .get_provider_by_id("b", app.as_str())
                            .unwrap()
                            .unwrap()
                            .settings_config,
                        intended.settings_config
                    );
                    assert_eq!(
                        crate::proxy::auto_strategy::get_model_pref_checked(
                            &fixture.state.db,
                            app.as_str()
                        )
                        .unwrap()
                        .as_deref(),
                        Some("selected-model")
                    );
                    assert_eq!(
                        crate::proxy::application_routing::chain_ids(
                            &fixture.state.db,
                            app.as_str()
                        )
                        .unwrap(),
                        vec!["b", "a"]
                    );
                    assert_eq!(
                        fixture
                            .state
                            .db
                            .get_current_provider(app.as_str())
                            .unwrap()
                            .as_deref(),
                        Some(if disposition == "direct" { "b" } else { "a" })
                    );
                    let mode = super::current::validate_known_mode(
                        &DeviceStore::for_device(),
                        &fixture.state.db.secret_session().read().unwrap(),
                        &app,
                    )
                    .unwrap();
                    assert_eq!(mode.attached, disposition == "proxy");
                    for file in &pending.files {
                        assert_eq!(
                            crate::live::engine::digest(
                                read_current(&file.path).unwrap().as_deref()
                            ),
                            file.planned
                        );
                    }
                }
            }
        }
        println!(
            "PASS R3 combined row/model/order restart matrix {}",
            app.as_str()
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn order_only_changes_use_zero_file_intent_and_recovery_for_all_apps() {
    for app in super::controller::PROXY_APPS {
        for point in ["pending", "target"] {
            let fixture = Fixture::for_app(app.clone());
            crate::proxy::auto_strategy::set_model_pref(
                &fixture.state.db,
                app.as_str(),
                Some("retained-preference"),
            )
            .unwrap();
            let before = super::controller::files(&app)
                .unwrap()
                .iter()
                .map(|f| read_current(&f.path).unwrap())
                .collect::<Vec<_>>();
            {
                let _fault = Fault::at(point);
                assert!(crate::services::application_selection::apply_order_change(
                    &fixture.state,
                    &app,
                    &selected_order()
                )
                .is_err());
            }
            let pending = state::pending(
                &DeviceStore::for_device(),
                &fixture.state.db.secret_session().read().unwrap(),
                app.as_str(),
            )
            .unwrap()
            .unwrap();
            assert!(pending.files.is_empty());
            assert!(
                pending.target.pointer.is_none()
                    && pending.target.state.is_none()
                    && pending.target.written.is_none()
                    && pending.target.saved_row.is_none()
                    && pending.target.model_preference.is_none()
            );
            assert!(crate::services::application_selection::apply_order_change(
                &fixture.state,
                &app,
                &selected_order()
            )
            .is_err());
            assert_eq!(
                super::controller::recover_locked(&fixture.state.proxy_service, &app).unwrap(),
                Some(if point == "pending" {
                    super::operation::RecoveryOutcome::Discarded
                } else {
                    super::operation::RecoveryOutcome::RolledForward
                })
            );
            assert_eq!(
                before,
                super::controller::files(&app)
                    .unwrap()
                    .iter()
                    .map(|f| read_current(&f.path).unwrap())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                crate::proxy::auto_strategy::get_model_pref_checked(
                    &fixture.state.db,
                    app.as_str()
                )
                .unwrap()
                .as_deref(),
                Some("retained-preference")
            );
            if point == "target" {
                assert_eq!(
                    crate::proxy::application_routing::chain_ids(&fixture.state.db, app.as_str())
                        .unwrap(),
                    vec!["b", "a"]
                );
            }
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn changed_order_blocks_replay_before_remaining_codex_files() {
    let fixture = selection_fixture_for(AppType::Codex);
    {
        let _fault = Fault::at("published:1");
        assert!(crate::services::application_selection::select_with_commit(
            &fixture.state,
            &AppType::Codex,
            &selected_model(),
            Some(&selected_order())
        )
        .is_err());
    }
    let pending = state::pending(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        "codex",
    )
    .unwrap()
    .unwrap();
    assert!(
        pending
            .files
            .iter()
            .skip(2)
            .any(|file| file.pre != file.planned
                && crate::live::engine::digest(read_current(&file.path).unwrap().as_deref())
                    == file.pre),
        "a real later file must still await replay"
    );
    crate::services::order_profiles::apply_order(
        &fixture.state.db,
        "codex",
        "default",
        &["a".into()],
    )
    .unwrap();
    let before = super::controller::files(&AppType::Codex)
        .unwrap()
        .iter()
        .map(|f| read_current(&f.path).unwrap())
        .collect::<Vec<_>>();
    assert!(
        super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Codex).is_err()
    );
    assert_eq!(
        before,
        super::controller::files(&AppType::Codex)
            .unwrap()
            .iter()
            .map(|f| read_current(&f.path).unwrap())
            .collect::<Vec<_>>()
    );
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
    assert_eq!(
        crate::proxy::application_routing::chain_ids(&fixture.state.db, "codex").unwrap(),
        vec!["a"]
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn actual_model_or_order_sql_failure_retains_combined_target_until_recovery() {
    for key in [
        "auto_mode_model_claude",
        "application_order_profile_current_claude",
    ] {
        let fixture = selection_fixture();
        fixture.state.db.conn.lock().unwrap().execute_batch(&format!("CREATE TRIGGER reject_r3 BEFORE INSERT ON settings WHEN NEW.key='{key}' BEGIN SELECT RAISE(ABORT, 'synthetic rejected target'); END;")).unwrap();
        assert!(crate::services::application_selection::select_with_commit(
            &fixture.state,
            &AppType::Claude,
            &selected_model(),
            Some(&selected_order())
        )
        .is_err());
        assert!(fixture.pending().is_some());
        fixture
            .state
            .db
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_r3")
            .unwrap();
        assert_eq!(
            super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude)
                .unwrap(),
            Some(super::operation::RecoveryOutcome::RolledForward)
        );
        assert!(fixture.pending().is_none());
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref_checked(&fixture.state.db, "claude")
                .unwrap()
                .as_deref(),
            Some("selected-model")
        );
        assert_eq!(
            crate::proxy::application_routing::chain_ids(&fixture.state.db, "claude").unwrap(),
            vec!["b", "a"]
        );
        crate::services::application_selection::select_with_commit(
            &fixture.state,
            &AppType::Claude,
            &crate::services::application_selection::TierSelection {
                provider_id: "b".into(),
                model: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref_checked(&fixture.state.db, "claude")
                .unwrap()
                .as_deref(),
            Some("")
        );
    }
}

#[cfg_attr(test, test)]
fn explicit_model_actions_round_trip_without_null_collapse() {
    for action in [
        state::ModelPreferenceAction::Clear {},
        state::ModelPreferenceAction::Set {
            model: "selected-model".into(),
        },
    ] {
        let target = state::PendingTarget {
            model_preference: Some(action),
            ..Default::default()
        };
        assert_eq!(
            serde_json::from_slice::<state::PendingTarget>(&serde_json::to_vec(&target).unwrap())
                .unwrap(),
            target
        );
    }
    assert!(serde_json::from_value::<state::PendingTarget>(
        json!({"model_preference":{"action":"unknown"}})
    )
    .is_err());
    assert!(serde_json::from_value::<state::PendingTarget>(
        json!({"model_preference":{"action":"clear","future":true}})
    )
    .is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn selected_model_survives_legacy_common_snippet_normalization() {
    let mut failed = Vec::new();
    for app in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        let result = std::panic::catch_unwind(|| {
            let fixture = selection_fixture_for(app.clone());
            let mut row = fixture
                .state
                .db
                .get_provider_by_id("b", app.as_str())
                .unwrap()
                .unwrap();
            row.meta
                .get_or_insert_with(Default::default)
                .common_config_enabled = Some(true);
            fixture.state.db.save_provider(app.as_str(), &row).unwrap();
            let snippet = match app {
                AppType::Claude => r#"{"env":{"ANTHROPIC_MODEL":"selected-model"}}"#.to_string(),
                AppType::Codex => "model = \"selected-model\"\n".to_string(),
                AppType::Gemini => r#"{"GEMINI_MODEL":"selected-model"}"#.to_string(),
                _ => unreachable!(),
            };
            fixture
                .state
                .db
                .set_config_snippet(app.as_str(), Some(snippet))
                .unwrap();
            crate::services::application_selection::select_with_commit(
                &fixture.state,
                &app,
                &selected_model(),
                None,
            )
            .unwrap();
            let live = ProviderService::read_live_settings(app.clone()).unwrap();
            assert_eq!(
                crate::relay::provider_config::selected_model(&app, &live).as_deref(),
                Some("selected-model"),
                "{app:?}: committed model preference must match actual native model"
            );
            let mut saved = fixture
                .state
                .db
                .get_provider_by_id("b", app.as_str())
                .unwrap()
                .unwrap();
            saved.name = "edited row".into();
            ProviderService::update(&fixture.state, app.clone(), None, saved).unwrap();
            let live = ProviderService::read_live_settings(app.clone()).unwrap();
            assert_eq!(
                crate::relay::provider_config::selected_model(&app, &live).as_deref(),
                Some("selected-model")
            );
        });
        if result.is_err() {
            failed.push(app);
        }
    }
    assert!(
        failed.is_empty(),
        "legacy normalization erased selected models: {failed:?}"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn modern_selection_preserves_claude_plugin_post_switch_hook() {
    let fixture = Fixture::claude();
    let mut settings = crate::settings::get_settings();
    settings.enable_claude_plugin_integration = true;
    crate::settings::update_settings(settings).unwrap();
    let path = crate::claude_plugin::claude_config_path().unwrap();
    assert!(path.starts_with(fixture._home.path()));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, br#"{"unowned":"keep"}"#).unwrap();
    crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Claude,
        &crate::services::application_selection::TierSelection {
            provider_id: "b".into(),
            model: None,
        },
        None,
    )
    .unwrap();
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(config["primaryApiKey"], "any");
    assert_eq!(config["unowned"], "keep");
    let mut official = Provider::with_id(
        "official".into(),
        "Official".into(),
        json!({"env":{}}),
        None,
    );
    official.category = Some("official".into());
    fixture.state.db.save_provider("claude", &official).unwrap();
    crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Claude,
        &crate::services::application_selection::TierSelection {
            provider_id: "official".into(),
            model: None,
        },
        None,
    )
    .unwrap();
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(config.get("primaryApiKey").is_none());
    assert_eq!(config["unowned"], "keep");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let result = crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Claude,
        &crate::services::application_selection::TierSelection {
            provider_id: "b".into(),
            model: None,
        },
        None,
    )
    .unwrap();
    assert!(result
        .warnings
        .iter()
        .any(|warning| warning.starts_with("claude_plugin_sync_failed:")));
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn bare_legacy_order_entry_cannot_write_through_modern_pending() {
    let fixture = Fixture::claude();
    {
        let _fault = Fault::at("published:0");
        assert!(ProviderService::switch(&fixture.state, AppType::Claude, "b").is_err());
    }
    let before = routing_settings(&fixture.state.db, &AppType::Claude);
    let pending = fixture.pending().unwrap();
    assert!(
        crate::proxy::application_routing::set_order(
            &fixture.state.db,
            "claude",
            &["b".into(), "a".into()]
        )
        .is_err(),
        "the still-used legacy set-priority entry cannot bypass app intent"
    );
    assert_eq!(
        routing_settings(&fixture.state.db, &AppType::Claude),
        before
    );
    assert_eq!(fixture.pending().unwrap(), pending);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn current_priority_service_retains_named_owner_and_refuses_pending() {
    let fixture = Fixture::claude();
    crate::services::order_profiles::save(
        &fixture.state.db,
        "claude",
        "work",
        &["a".into(), "b".into()],
    )
    .unwrap();
    crate::services::order_profiles::apply_order(
        &fixture.state.db,
        "claude",
        "work",
        &["a".into(), "b".into()],
    )
    .unwrap();
    {
        let _fault = Fault::at("pending");
        assert!(
            crate::services::application_selection::apply_current_order_change(
                &fixture.state,
                &AppType::Claude,
                &["b".into(), "a".into()]
            )
            .is_err()
        );
    }
    let pending = fixture.pending().unwrap();
    assert!(pending.files.is_empty());
    assert_eq!(
        pending.target.routing_order.as_ref().unwrap().profile_name,
        "work"
    );
    assert!(
        crate::services::application_selection::apply_current_order_change(
            &fixture.state,
            &AppType::Claude,
            &["a".into(), "b".into()]
        )
        .is_err()
    );
    assert_eq!(fixture.pending().unwrap(), pending);
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    crate::services::application_selection::apply_current_order_change(
        &fixture.state,
        &AppType::Claude,
        &["b".into(), "a".into()],
    )
    .unwrap();
    assert_eq!(
        crate::services::order_profiles::get(&fixture.state.db, "claude")
            .unwrap()
            .current,
        "work"
    );
    assert_eq!(
        crate::proxy::application_routing::chain_ids(&fixture.state.db, "claude").unwrap(),
        vec!["b", "a"]
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn r3_clear_model_action_survives_actual_restart_and_recovery() {
    for app in [AppType::Claude, AppType::Codex] {
        let mut fixture = selection_fixture_for(app.clone());
        crate::services::application_selection::select_with_commit(
            &fixture.state,
            &app,
            &selected_model(),
            None,
        )
        .unwrap();
        {
            let _fault = Fault::at("target");
            assert!(crate::services::application_selection::select_with_commit(
                &fixture.state,
                &app,
                &crate::services::application_selection::TierSelection {
                    provider_id: "b".into(),
                    model: None
                },
                None
            )
            .is_err());
        }
        let pending = state::pending(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            app.as_str(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            pending.target.model_preference,
            Some(state::ModelPreferenceAction::Clear {})
        );
        fixture.state = AppState::new(fixture.state.db.clone()).unwrap();
        assert_eq!(
            super::controller::recover_locked(&fixture.state.proxy_service, &app).unwrap(),
            Some(super::operation::RecoveryOutcome::RolledForward)
        );
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref_checked(&fixture.state.db, app.as_str())
                .unwrap()
                .as_deref(),
            Some("")
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn r3_selection_reports_preserved_external_codex_catalog() {
    let fixture = selection_fixture_for(AppType::Codex);
    let text = std::fs::read_to_string(&fixture.path).unwrap();
    let text = crate::codex_config::update_codex_toml_field(
        &text,
        "model_catalog_json",
        "unclaimed-external.json",
    )
    .unwrap();
    std::fs::write(&fixture.path, text).unwrap();
    let external = fixture
        .path
        .parent()
        .unwrap()
        .join("unclaimed-external.json");
    std::fs::write(&external, b"opaque synthetic external catalog").unwrap();
    let result = crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Codex,
        &selected_model(),
        None,
    )
    .unwrap();
    assert!(result.warnings.iter().any(
        |warning| warning == crate::services::provider::codex_direct::CATALOG_PRESERVED_WARNING
    ));
    let doc = std::fs::read_to_string(&fixture.path)
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert_eq!(
        doc["model_catalog_json"].as_str(),
        Some("unclaimed-external.json")
    );
    assert_eq!(
        std::fs::read(&external).unwrap(),
        b"opaque synthetic external catalog"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn r3_target_readback_finishes_with_a_waiting_vault_writer() {
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let operation = std::thread::spawn(move || {
        let fixture = selection_fixture();
        let db = &fixture.state.db;
        let before = db.get_provider_by_id("b", "claude").unwrap().unwrap();
        let mut after = before.clone();
        after.name = "selected synthetic row".into();
        let order = selected_order();
        let (order_before, order_planned) = crate::services::order_profiles::prepare_apply(
            db,
            "claude",
            &order.profile_name,
            &order.provider_ids,
        )
        .unwrap();
        let target = state::PendingTarget {
            pointer: Some("b".into()),
            saved_row: Some(state::SavedRow {
                before: crate::database::Database::provider_update_digest(&before).unwrap(),
                provider: crate::database::Database::provider_update_value(&after).unwrap(),
                clear_model_preference: false,
            }),
            model_preference: Some(state::ModelPreferenceAction::Set {
                model: "selected-model".into(),
            }),
            routing_order: Some(state::RoutingOrderTarget {
                profile_name: order.profile_name,
                provider_ids: order.provider_ids,
                before: order_before,
                planned: order_planned,
            }),
            ..Default::default()
        };
        let vault = db.secret_session().read().unwrap();
        let session = db.secrets.clone();
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            waiting_tx.send(()).unwrap();
            let _write = session.write().unwrap();
            acquired_tx.send(()).unwrap();
        });
        waiting_rx.recv().unwrap();
        assert!(acquired_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        super::operation::commit_target(
            db,
            db.secret_session(),
            &DeviceStore::for_device(),
            &vault,
            &AppType::Claude,
            &target,
        )
        .unwrap();
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref_checked(db, "claude")
                .unwrap()
                .as_deref(),
            Some("selected-model")
        );
        assert_eq!(
            crate::proxy::application_routing::chain_ids(db, "claude").unwrap(),
            vec!["b", "a"]
        );
        drop(vault);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        writer.join().unwrap();
        finished_tx.send(()).unwrap();
    });
    finished_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("R3 commit/readback cannot recursively acquire the queued vault guard");
    operation.join().unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plugin_post_selection_follows_attached_route_and_detached_direct_owner() {
    let fixture = Fixture::claude();
    let mut settings = crate::settings::get_settings();
    settings.enable_claude_plugin_integration = true;
    crate::settings::update_settings(settings).unwrap();
    crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Claude,
        &crate::services::application_selection::TierSelection {
            provider_id: "a".into(),
            model: None,
        },
        None,
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
    fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .set_takeover_for_app("claude", false),
        )
        .unwrap();
    let mut official = Provider::with_id(
        "official".into(),
        "Official".into(),
        json!({"env":{}}),
        None,
    );
    official.category = Some("official".into());
    fixture.state.db.save_provider("claude", &official).unwrap();
    crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Claude,
        &crate::services::application_selection::TierSelection {
            provider_id: "official".into(),
            model: None,
        },
        None,
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
    crate::services::application_selection::select_with_commit(
        &fixture.state,
        &AppType::Claude,
        &crate::services::application_selection::TierSelection {
            provider_id: "b".into(),
            model: None,
        },
        None,
    )
    .unwrap();
    assert_eq!(
        fixture
            .state
            .db
            .get_current_provider("claude")
            .unwrap()
            .as_deref(),
        Some("official")
    );
    let path = crate::claude_plugin::claude_config_path().unwrap();
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        config["primaryApiKey"], "any",
        "attached third-party route owns native plugin integration"
    );
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore_keep_state())
        .unwrap();
    ProviderService::sync_claude_plugin_integration(&fixture.state).unwrap();
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        config.get("primaryApiKey").is_none(),
        "detached mode follows restored official Direct owner"
    );
}

/// Same local axum upstream pattern as proxy::auto_mode_e2e_tests, exercised
/// through the real AppState-owned server and modern mode controller.
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn modern_http_fallback_preserves_direct_and_obeys_applied_chain() {
    // Match the application bootstrap, which the standalone verifier does not run.
    let _ = rustls::crypto::ring::default_provider().install_default();
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    for disposition in [
        "enabled",
        "disabled",
        "excluded",
        "restart",
        "pending",
        "source17",
        "same-listener",
    ] {
        let fixture = Fixture::for_app_with_schema(AppType::Claude, disposition != "source17");
        let fail_a = Arc::new(AtomicBool::new(false));
        let a_hits = Arc::new(AtomicUsize::new(0));
        let b_hits = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let mock = fixture.runtime.block_on(async {
            let failure = fail_a.clone();
            let hits = b_hits.clone();
            let first_hits = a_hits.clone();
            let entered = entered.clone();
            let release = release.clone();
            let router = axum::Router::new().fallback(move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let failure = failure.clone();
                let hits = hits.clone();
                let first_hits = first_hits.clone();
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    let id = if uri.path().starts_with("/a/") { "a" } else { "b" };
                    assert_eq!(headers.get("authorization").unwrap(), format!("Bearer synthetic-key-{id}").as_str());
                    assert_ne!(headers.get("x-api-key").and_then(|v| v.to_str().ok()), Some("client-key-must-not-be-forwarded"));
                    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert!(request.get("model").and_then(|v| v.as_str()).is_some());
                    if id == "a" { first_hits.fetch_add(1, Ordering::SeqCst); }
                    if id == "b" {
                        let prior_hits = hits.fetch_add(1, Ordering::SeqCst);
                        if prior_hits == 0 && matches!(disposition, "restart" | "same-listener") { entered.notify_one(); release.notified().await; }
                    }
                    if id == "a" && failure.load(Ordering::SeqCst) {
                        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({"type":"error","error":{"type":"api_error","message":"synthetic failure"}})));
                    }
                    (axum::http::StatusCode::OK, axum::Json(json!({"id":"msg_synthetic","type":"message","role":"assistant","model":"synthetic-model-a","content":[{"type":"text","text":id}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}})))
                }
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
            (port, task)
        });
        for id in ["a", "b"] {
            let mut row = fixture
                .state
                .db
                .get_provider_by_id(id, "claude")
                .unwrap()
                .unwrap();
            row.settings_config["env"]["ANTHROPIC_BASE_URL"] =
                json!(format!("http://127.0.0.1:{}/{id}", mock.0));
            fixture.state.db.save_provider("claude", &row).unwrap();
        }
        let ids = if disposition == "excluded" {
            vec!["a".into()]
        } else {
            vec!["a".into(), "b".into()]
        };
        crate::services::application_selection::apply_current_order_change(
            &fixture.state,
            &AppType::Claude,
            &ids,
        )
        .unwrap();
        ProviderService::switch(&fixture.state, AppType::Claude, "a").unwrap();
        fixture.runtime.block_on(async {
            let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
            global.listen_address = "127.0.0.1".into();
            global.listen_port = 0;
            fixture.state.db.update_global_proxy_config(global).await.unwrap();
            fixture.state.proxy_service.set_takeover_for_app("claude", true).await.unwrap();
            fixture.state.proxy_service.set_failover_for_app("claude", disposition != "disabled").await.unwrap();
            let mut config = fixture.state.db.get_proxy_config_for_app("claude").await.unwrap();
            config.max_retries = 1;
            config.circuit_failure_threshold = 1;
            config.circuit_timeout_seconds = 0;
            fixture.state.db.update_proxy_config_for_app(config).await.unwrap();
            let port = fixture.state.proxy_service.start().await.unwrap().port;
            let client = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(10)).build().unwrap();
            let send = || client.post(format!("http://127.0.0.1:{port}/v1/messages"))
                .header("x-api-key", "client-key-must-not-be-forwarded")
                .header("anthropic-version", "2023-06-01")
                .json(&json!({"model":"synthetic-model-a","max_tokens":16,"stream":false,"messages":[{"role":"user","content":"hi"}]})).send();
            let initial = send().await.unwrap();
            assert!(initial.status().is_success(), "initial {disposition}: {}", initial.text().await.unwrap());
            fail_a.store(true, Ordering::SeqCst);
            if disposition == "pending" {
                let _fault = Fault::at("pending");
                assert!(fixture.state.proxy_service.hot_switch_provider("claude", "b").await.is_err());
                assert!(fixture.pending().is_some());
            }
            let journal_before = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
            let live_before = read_current(&fixture.path).unwrap();
            let response = if matches!(disposition, "restart" | "same-listener") {
                let request = tokio::spawn(send());
                tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified()).await.unwrap();
                let old_manager = fixture.state.proxy_service.active_failover_manager_for_test().await;
                let old_identity = fixture.state.proxy_service.request_identity("claude").unwrap();
                if disposition == "same-listener" {
                    // A second admitted app keeps this listener alive throughout.
                    let gemini = Provider::with_id("g".into(), "synthetic-gemini".into(), json!({"env":{"GEMINI_API_KEY":"synthetic-key","GOOGLE_GEMINI_BASE_URL":"https://gemini.example.invalid"},"config":{"model":{"name":"synthetic-model"}}}), None);
                    fixture.state.db.save_provider("gemini", &gemini).unwrap();
                    let mut other = gemini.clone();
                    other.id = "g2".into();
                    fixture.state.db.save_provider("gemini", &other).unwrap();
                    fixture.state.db.set_current_provider("gemini", "g").unwrap();
                    crate::settings::set_current_provider(&AppType::Gemini, Some("g")).unwrap();
                    state::update(&DeviceStore::for_device(), &fixture.state.db.secret_session().read().unwrap(), |live| {
                        live.apps.entry("gemini".into()).or_default().mode = Some(Mode::Direct);
                        Ok(())
                    }).unwrap();
                    fixture.state.proxy_service.set_takeover_for_app("gemini", true).await.unwrap();
                    fixture.state.proxy_service.set_failover_for_app("gemini", true).await.unwrap();
                    let other_identity = fixture.state.proxy_service.request_identity("gemini").unwrap();
                    fixture.state.proxy_service.set_takeover_for_app("claude", false).await.unwrap();
                    fixture.state.proxy_service.set_takeover_for_app("claude", true).await.unwrap();
                    fixture.state.proxy_service.set_failover_for_app("claude", true).await.unwrap();
                    assert!(old_manager.same_instance(fixture.state.proxy_service.active_failover_manager_for_test().await.as_ref()));
                    assert!(!fixture.state.proxy_service.request_identity_is_current("claude", Some(&old_identity)).unwrap());
                    assert!(fixture.state.proxy_service.request_identity_is_current("gemini", Some(&other_identity)).unwrap());
                    assert!(old_manager.try_switch(#[cfg(feature = "gui")] None, "gemini", "g2", "g2", "g", Some(&other_identity)).await.unwrap());
                } else {
                    fixture.state.proxy_service.stop_with_restore_keep_state().await.unwrap();
                    fixture.state.proxy_service.recover_from_crash().await.unwrap();
                    assert!(!old_manager.try_switch(#[cfg(feature = "gui")] None, "claude", "b", "b", "a", Some(&old_identity)).await.unwrap());
                }
                release.notify_one();
                request.await.unwrap().unwrap()
            } else { send().await.unwrap() };
            let status = response.status();
            let body = response.text().await.unwrap();
            if disposition == "enabled" || disposition == "source17" {
                assert!(status.is_success(), "fallback: {status} {body}");
                assert!(body.contains("\"text\":\"b\""), "{body}");
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        if crate::proxy::application_routing::current_provider_id_checked(&fixture.state.db, "claude").ok().flatten().as_deref() == Some("b") { break; }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }).await.expect("successful real HTTP fallback commits route");
                assert_eq!(b_hits.load(Ordering::SeqCst), 1);
            } else if matches!(disposition, "restart" | "same-listener") {
                assert!(status.is_success(), "old request may finish: {status} {body}");
                let stale_commit = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    loop {
                        if crate::proxy::application_routing::current_provider_id_checked(&fixture.state.db, "claude").ok().flatten().as_deref() == Some("b") { break; }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }).await;
                assert!(stale_commit.is_err(), "old HTTP request must not change reattached same-ID route: {disposition}");
                if disposition == "same-listener" {
                    assert_eq!(crate::proxy::application_routing::current_provider_id_checked(&fixture.state.db, "claude").unwrap().as_deref(), Some("a"));
                    let fresh = send().await.unwrap();
                    assert!(fresh.status().is_success());
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        loop {
                            if crate::proxy::application_routing::current_provider_id_checked(&fixture.state.db, "claude").ok().flatten().as_deref() == Some("b") { break; }
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    }).await.expect("new attachment request can still commit fallback");
                }
            } else {
                assert!(!status.is_success(), "{disposition}: {status} {body}");
                assert_eq!(b_hits.load(Ordering::SeqCst), 0);
            }
            if disposition == "pending" {
                assert_eq!(a_hits.load(Ordering::SeqCst), 1, "pending request must fail before upstream I/O");
                assert!(fixture.pending().is_some());
                assert_eq!(std::fs::read(DeviceStore::for_device().state_path()).unwrap(), journal_before);
                assert_eq!(read_current(&fixture.path).unwrap(), live_before);
                tokio::task::block_in_place(|| super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude)).unwrap();
            }
            if disposition == "source17" {
                let mut replacement = fixture.state.proxy_service.get_config().await.unwrap();
                replacement.listen_port = 0;
                fixture.state.proxy_service.update_config(&replacement).await.unwrap();
                assert_eq!(fixture.state.db.get_current_provider("claude").unwrap().as_deref(), Some("b"));
                assert_eq!(crate::settings::get_current_provider(&AppType::Claude).as_deref(), Some("b"));
                fixture.state.proxy_service.stop_with_restore().await.unwrap();
                let restored: serde_json::Value = serde_json::from_slice(&read_current(&fixture.path).unwrap().unwrap()).unwrap();
                assert_eq!(restored["env"]["ANTHROPIC_BASE_URL"], json!(format!("http://127.0.0.1:{}/b", mock.0)));
                return;
            }
            let mode = super::current::validate_known_mode(&DeviceStore::for_device(), &fixture.state.db.secret_session().read().unwrap(), &AppType::Claude).unwrap();
            assert!(mode.is_proxy() && mode.attached);
            assert_eq!(mode.proxy_route.as_deref(), Some(if matches!(disposition, "enabled" | "same-listener") { "b" } else { "a" }));
            assert_eq!(fixture.state.db.get_current_provider("claude").unwrap().as_deref(), Some("a"));
            assert_eq!(crate::settings::get_current_provider(&AppType::Claude).as_deref(), Some("a"));
            assert!(fixture.pending().is_none());
            fixture.state.proxy_service.stop_with_restore().await.unwrap();
            let restored: serde_json::Value = serde_json::from_slice(&read_current(&fixture.path).unwrap().unwrap()).unwrap();
            assert_eq!(restored["env"]["ANTHROPIC_BASE_URL"], json!(format!("http://127.0.0.1:{}/a", mock.0)));
        });
        mock.1.abort();
        println!("PASS actual HTTP {disposition}");
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_http() -> Result<(), AppError> {
    listener_configuration_restart_waits_for_active_failover_commit();
    modern_http_fallback_preserves_direct_and_obeys_applied_chain();
    synchronous_disable_and_partial_recovery_never_revive_old_requests();
    println!("PASS actual modern HTTP routing/fallback/direct isolation/applied chain");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn listener_configuration_restart_waits_for_active_failover_commit() {
    let fixture = Fixture::claude();
    fixture.runtime.block_on(async {
        let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
        global.listen_address = "127.0.0.1".into();
        global.listen_port = 0;
        fixture
            .state
            .db
            .update_global_proxy_config(global)
            .await
            .unwrap();
        fixture
            .state
            .proxy_service
            .set_takeover_for_app("claude", true)
            .await
            .unwrap();
        let service = fixture.state.proxy_service.clone();
        let manager = service.active_failover_manager_for_test().await;
        let guard = service
            .lock_active_failover("claude", &manager)
            .await
            .unwrap();
        let mut config = service.get_config().await.unwrap();
        let before = config.listen_port;
        assert_ne!(before, 0);
        config.listen_port = 0;
        let update_service = service.clone();
        let update = tokio::spawn(async move { update_service.update_config(&config).await });
        let changed_while_commit =
            tokio::time::timeout(std::time::Duration::from_millis(300), async {
                loop {
                    if service.get_config().await.unwrap().listen_port != before {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_ok();
        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(5), update)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        service.stop_with_restore().await.unwrap();
        assert!(
            !changed_while_commit,
            "configuration restart cannot replace listener facts during admitted failover commit"
        );
    });
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn pending_operation_refuses_listener_reconfiguration_before_effects() {
    let fixture = Fixture::claude();
    fixture.runtime.block_on(async {
        let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
        global.listen_address = "127.0.0.1".into();
        global.listen_port = 0;
        fixture
            .state
            .db
            .update_global_proxy_config(global)
            .await
            .unwrap();
        fixture
            .state
            .proxy_service
            .set_takeover_for_app("claude", true)
            .await
            .unwrap();
        {
            let _fault = Fault::at("pending");
            assert!(fixture
                .state
                .proxy_service
                .hot_switch_provider("claude", "b")
                .await
                .is_err());
        }
        assert!(fixture.pending().is_some());
        let before = fixture.state.proxy_service.get_config().await.unwrap();
        let live = read_current(&fixture.path).unwrap();
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        let manager = fixture
            .state
            .proxy_service
            .active_failover_manager_for_test()
            .await;
        let mut next = before.clone();
        next.listen_port = 0;
        let result = fixture.state.proxy_service.update_config(&next).await;
        assert!(
            result.is_err(),
            "pending listener reconfiguration must fail before side effects"
        );
        assert_eq!(
            fixture
                .state
                .proxy_service
                .get_config()
                .await
                .unwrap()
                .listen_port,
            before.listen_port
        );
        assert!(manager.same_instance(
            fixture
                .state
                .proxy_service
                .active_failover_manager_for_test()
                .await
                .as_ref()
        ));
        assert_eq!(read_current(&fixture.path).unwrap(), live);
        assert_eq!(
            std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
        tokio::task::block_in_place(|| {
            super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude)
        })
        .unwrap();
        fixture
            .state
            .proxy_service
            .stop_with_restore()
            .await
            .unwrap();
    });
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_reconfiguration() -> Result<(), AppError> {
    runtime_only_configuration_does_not_apply_provider_files();
    let admission =
        std::panic::catch_unwind(pending_operation_refuses_listener_reconfiguration_before_effects);
    let projection = std::panic::catch_unwind(
        listener_reconfiguration_reprojects_through_original_mode_contract,
    );
    assert!(
        admission.is_ok() && projection.is_ok(),
        "listener reconfiguration admission and mode contract"
    );
    listener_reconfiguration_preserves_unattached_and_unknown_modes();
    failed_listener_reprojection_keeps_pending_and_running_listener();
    listener_bind_failure_can_retry_saved_configuration();
    listener_reconfiguration_reports_one_app_failure_without_undoing_another();
    println!("PASS listener reconfiguration pending, four-app contracts/recovery, runtime-only preservation, independent partial results, saved-config/bind retry, detached/unknown modes");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn listener_reconfiguration_reprojects_through_original_mode_contract() {
    for app in super::controller::PROXY_APPS {
        let fixture = Fixture::for_app(app.clone());
        fixture.runtime.block_on(async {
            let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
            global.listen_address = "127.0.0.1".into();
            global.listen_port = 0;
            fixture
                .state
                .db
                .update_global_proxy_config(global)
                .await
                .unwrap();
            fixture
                .state
                .proxy_service
                .set_takeover_for_app(app.as_str(), true)
                .await
                .unwrap();
            let before = super::current::validate_known_mode(
                &DeviceStore::for_device(),
                &fixture.state.db.secret_session().read().unwrap(),
                &app,
            )
            .unwrap();
            let mut config = fixture.state.proxy_service.get_config().await.unwrap();
            config.listen_port = 0;
            fixture
                .state
                .proxy_service
                .update_config(&config)
                .await
                .unwrap();
            let after = super::current::validate_known_mode(
                &DeviceStore::for_device(),
                &fixture.state.db.secret_session().read().unwrap(),
                &app,
            )
            .unwrap();
            assert_ne!(
                before.contract,
                after.contract,
                "{} listener URL must commit its new mode contract",
                app.as_str()
            );
            assert!(after.is_proxy() && after.attached);
            assert_eq!(after.proxy_route.as_deref(), Some("a"));
            fixture
                .state
                .proxy_service
                .stop_with_restore()
                .await
                .unwrap();
        });
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn listener_reconfiguration_preserves_unattached_and_unknown_modes() {
    for disposition in ["direct", "detached", "missing", "unknown"] {
        let fixture = Fixture::claude();
        fixture.runtime.block_on(async {
            let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
            global.listen_address = "127.0.0.1".into();
            global.listen_port = 0;
            fixture
                .state
                .db
                .update_global_proxy_config(global)
                .await
                .unwrap();
            if disposition == "detached" {
                fixture
                    .state
                    .proxy_service
                    .set_takeover_for_app("claude", true)
                    .await
                    .unwrap();
                fixture
                    .state
                    .proxy_service
                    .stop_with_restore_keep_state()
                    .await
                    .unwrap();
            }
            fixture.state.proxy_service.start_for_mode().await.unwrap();
            let store = DeviceStore::for_device();
            if disposition == "missing" {
                std::fs::remove_file(store.state_path()).unwrap();
            } else if disposition == "unknown" {
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
            }
            let journal = read_current(&store.state_path()).unwrap();
            let live = read_current(&fixture.path).unwrap();
            let before = fixture.state.proxy_service.get_config().await.unwrap();
            let mut next = before.clone();
            next.listen_port = 0;
            let result = fixture.state.proxy_service.update_config(&next).await;
            assert_eq!(
                result.is_ok(),
                matches!(disposition, "direct" | "detached"),
                "{disposition}: {result:?}"
            );
            if result.is_err() {
                assert_eq!(
                    fixture
                        .state
                        .proxy_service
                        .get_config()
                        .await
                        .unwrap()
                        .listen_port,
                    before.listen_port
                );
            }
            assert_eq!(read_current(&store.state_path()).unwrap(), journal);
            assert_eq!(read_current(&fixture.path).unwrap(), live);
            fixture.state.proxy_service.stop().await.unwrap();
        });
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn failed_listener_reprojection_keeps_pending_and_running_listener() {
    for app in super::controller::PROXY_APPS {
        let fixture = Fixture::for_app(app.clone());
        fixture.runtime.block_on(async {
            let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
            global.listen_address = "127.0.0.1".into();
            global.listen_port = 0;
            fixture
                .state
                .db
                .update_global_proxy_config(global)
                .await
                .unwrap();
            fixture
                .state
                .proxy_service
                .set_takeover_for_app(app.as_str(), true)
                .await
                .unwrap();
            let mut next = fixture.state.proxy_service.get_config().await.unwrap();
            next.listen_port = 0;
            {
                let _fault = Fault::at(if app == AppType::Codex {
                    "published:1"
                } else {
                    "published:0"
                });
                let error = fixture
                    .state
                    .proxy_service
                    .update_config(&next)
                    .await
                    .expect_err(&format!(
                        "{} must interrupt after changed client file publication",
                        app.as_str()
                    ));
                assert!(error.contains(app.as_str()), "{error}");
            }
            assert!(fixture.state.proxy_service.is_running().await);
            assert!(state::pending(
                &DeviceStore::for_device(),
                &fixture.state.db.secret_session().read().unwrap(),
                app.as_str()
            )
            .unwrap()
            .is_some());
            assert!(crate::mode::operation::AppWrite::begin_mode(
                &fixture.state.proxy_service,
                &app
            )
            .is_err());
            tokio::task::block_in_place(|| {
                super::controller::recover_locked(&fixture.state.proxy_service, &app)
            })
            .unwrap();
            fixture
                .state
                .proxy_service
                .stop_with_restore()
                .await
                .unwrap();
        });
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn listener_reconfiguration_reports_one_app_failure_without_undoing_another() {
    let fixture = Fixture::claude();
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
            live.apps.entry("gemini".into()).or_default().mode = Some(Mode::Direct);
            Ok(())
        },
    )
    .unwrap();
    ProviderService::switch(&fixture.state, AppType::Gemini, "g").unwrap();
    fixture.runtime.block_on(async {
        let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
        global.listen_address = "127.0.0.1".into();
        global.listen_port = 0;
        fixture
            .state
            .db
            .update_global_proxy_config(global)
            .await
            .unwrap();
        for app in ["claude", "gemini"] {
            fixture
                .state
                .proxy_service
                .set_takeover_for_app(app, true)
                .await
                .unwrap();
        }
        let gemini_before = super::current::validate_known_mode(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            &AppType::Gemini,
        )
        .unwrap();
        let claude_before = read_current(&fixture.path).unwrap().unwrap();
        crate::config_file_io::write_durable(&fixture.path, b"{ invalid synthetic JSON").unwrap();
        let mut config = fixture.state.proxy_service.get_config().await.unwrap();
        config.listen_port = 0;
        let error = fixture
            .state
            .proxy_service
            .update_config(&config)
            .await
            .unwrap_err();
        assert!(error.contains("claude"), "{error}");
        assert!(fixture.state.proxy_service.is_running().await);
        let gemini_after = super::current::validate_known_mode(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            &AppType::Gemini,
        )
        .unwrap();
        assert_ne!(gemini_before.contract, gemini_after.contract);
        assert!(gemini_after.attached);
        assert_eq!(
            read_current(&fixture.path).unwrap().unwrap(),
            b"{ invalid synthetic JSON"
        );
        assert!(state::pending(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            "gemini"
        )
        .unwrap()
        .is_none());
        crate::config_file_io::write_durable(&fixture.path, &claude_before).unwrap();
        let old_contract = super::current::validate_known_mode(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            &AppType::Claude,
        )
        .unwrap()
        .contract;
        let committed = fixture.state.proxy_service.get_config().await.unwrap();
        fixture
            .state
            .proxy_service
            .update_config(&committed)
            .await
            .unwrap();
        let repaired = super::current::validate_known_mode(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            &AppType::Claude,
        )
        .unwrap();
        assert_ne!(
            old_contract, repaired.contract,
            "retry of saved configuration must repair the uncommitted app projection"
        );
        fixture
            .state
            .proxy_service
            .stop_with_restore()
            .await
            .unwrap();
    });
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn listener_bind_failure_can_retry_saved_configuration() {
    let fixture = Fixture::claude();
    fixture.runtime.block_on(async {
        let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
        global.listen_address = "127.0.0.1".into();
        global.listen_port = 0;
        fixture
            .state
            .db
            .update_global_proxy_config(global)
            .await
            .unwrap();
        fixture
            .state
            .proxy_service
            .set_takeover_for_app("claude", true)
            .await
            .unwrap();
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut next = fixture.state.proxy_service.get_config().await.unwrap();
        next.listen_port = occupied.local_addr().unwrap().port();
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        let live = read_current(&fixture.path).unwrap();
        assert!(fixture
            .state
            .proxy_service
            .update_config(&next)
            .await
            .is_err());
        assert_eq!(
            std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
        assert_eq!(read_current(&fixture.path).unwrap(), live);
        assert!(!fixture.state.proxy_service.is_running().await);
        drop(occupied);
        fixture
            .state
            .proxy_service
            .update_config(&next)
            .await
            .unwrap();
        assert!(
            fixture.state.proxy_service.is_running().await,
            "same saved configuration retry must actually start the replacement listener"
        );
        assert_ne!(read_current(&fixture.path).unwrap(), live);
        fixture
            .state
            .proxy_service
            .stop_with_restore()
            .await
            .unwrap();
    });
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn runtime_only_configuration_does_not_apply_provider_files() {
    for app in super::controller::PROXY_APPS {
        let fixture = Fixture::for_app(app.clone());
        fixture.runtime.block_on(async {
            let mut global = fixture.state.db.get_global_proxy_config().await.unwrap();
            global.listen_address = "127.0.0.1".into();
            global.listen_port = 0;
            fixture
                .state
                .db
                .update_global_proxy_config(global)
                .await
                .unwrap();
            fixture
                .state
                .proxy_service
                .set_takeover_for_app(app.as_str(), true)
                .await
                .unwrap();
            let original = read_current(&fixture.path).unwrap().unwrap();
            crate::config_file_io::write_durable(
                &fixture.path,
                b"unrelated external edit [ { invalid",
            )
            .unwrap();
            let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
            let mut config = fixture.state.proxy_service.get_config().await.unwrap();
            config.enable_logging = !config.enable_logging;
            fixture
                .state
                .proxy_service
                .update_config(&config)
                .await
                .expect("runtime-only preference cannot imply provider application");
            assert_eq!(
                read_current(&fixture.path).unwrap().unwrap(),
                b"unrelated external edit [ { invalid"
            );
            assert_eq!(
                std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
                journal
            );
            crate::config_file_io::write_durable(&fixture.path, &original).unwrap();
            fixture
                .state
                .proxy_service
                .stop_with_restore()
                .await
                .unwrap();
        });
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn synchronous_disable_and_partial_recovery_never_revive_old_requests() {
    for app in super::controller::PROXY_APPS {
        for partial in [false, true] {
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
            let manager = fixture.runtime.block_on(
                fixture
                    .state
                    .proxy_service
                    .active_failover_manager_for_test(),
            );
            let old = fixture
                .state
                .proxy_service
                .request_identity(app.as_str())
                .unwrap();
            if partial {
                {
                    let _fault = Fault::at(if app == AppType::Codex {
                        "published:1"
                    } else {
                        "published:0"
                    });
                    assert!(fixture
                        .state
                        .proxy_service
                        .disable_takeover_for_app_sync(&app)
                        .is_err());
                }
                assert!(!fixture
                    .state
                    .proxy_service
                    .request_identity_is_current(app.as_str(), Some(&old))
                    .unwrap());
                super::controller::recover_locked(&fixture.state.proxy_service, &app).unwrap();
            } else {
                fixture
                    .state
                    .proxy_service
                    .disable_takeover_for_app_sync(&app)
                    .unwrap();
            }
            fixture
                .runtime
                .block_on(
                    fixture
                        .state
                        .proxy_service
                        .set_takeover_for_app(app.as_str(), true),
                )
                .unwrap();
            fixture
                .state
                .db
                .set_proxy_flags_sync(app.as_str(), true, true)
                .unwrap();
            assert!(!fixture
                .runtime
                .block_on(manager.try_switch(
                    #[cfg(feature = "gui")]
                    None,
                    app.as_str(),
                    "b",
                    "b",
                    "a",
                    Some(&old)
                ))
                .unwrap());
            assert_eq!(
                crate::proxy::application_routing::current_provider_id_checked(
                    &fixture.state.db,
                    app.as_str()
                )
                .unwrap()
                .as_deref(),
                Some("a")
            );
            let fresh = fixture
                .state
                .proxy_service
                .request_identity(app.as_str())
                .unwrap();
            assert!(fixture
                .runtime
                .block_on(manager.try_switch(
                    #[cfg(feature = "gui")]
                    None,
                    app.as_str(),
                    "b",
                    "b",
                    "a",
                    Some(&fresh)
                ))
                .unwrap());
            fixture
                .runtime
                .block_on(fixture.state.proxy_service.stop_with_restore())
                .unwrap();
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn routing_read_model_exposes_owner_facts_without_recovery_or_secrets() {
    let legacy = Fixture::for_app_with_schema(AppType::Claude, false);
    assert!(super::current::read_view(&legacy.state.proxy_service, &AppType::Claude).is_none());
    drop(legacy);
    let fixture = Fixture::claude();
    let direct = super::current::read_view(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    assert_eq!(direct.status, "ready");
    assert_eq!(direct.mode, Some(Mode::Direct));
    assert_eq!(direct.attached, Some(false));
    assert_eq!(direct.current_provider_id.as_deref(), Some("a"));
    assert!(direct.can_write);
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
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .hot_switch_provider("claude", "b"),
        )
        .unwrap();
    let proxy = super::current::read_view(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    assert_eq!(proxy.current_provider_id.as_deref(), Some("b"));
    assert_eq!(proxy.direct_provider_id.as_deref(), Some("a"));
    assert_eq!(proxy.attached, Some(true));
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore_keep_state())
        .unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.start_for_mode())
        .unwrap();
    fixture
        .state
        .db
        .set_proxy_flags_sync("claude", true, true)
        .unwrap();
    let detached =
        super::current::read_view(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    assert_eq!(detached.mode, Some(Mode::Proxy));
    assert_eq!(
        detached.attached,
        Some(false),
        "compatibility flag plus listener is not attachment"
    );
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.recover_from_crash())
        .unwrap();
    {
        let _fault = Fault::at("published:0");
        assert!(fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .hot_switch_provider("claude", "a")
            )
            .is_err());
    }
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let live = read_current(&fixture.path).unwrap();
    for _ in 0..2 {
        let pending =
            super::current::read_view(&fixture.state.proxy_service, &AppType::Claude).unwrap();
        assert_eq!(pending.status, "pending");
        assert!(!pending.can_write);
        assert_eq!(pending.publication_started, Some(true));
        assert_eq!(pending.mode, None);
        assert_eq!(pending.current_provider_id, None);
        let public = serde_json::to_string(&pending).unwrap();
        for secret in [
            "synthetic-key",
            "auth",
            "staged",
            "files",
            fixture.path.to_str().unwrap(),
        ] {
            assert!(
                !public.contains(secret),
                "read DTO must not contain {secret}"
            );
        }
        assert_eq!(
            std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
        assert_eq!(read_current(&fixture.path).unwrap(), live);
    }
    assert!(fixture.pending().is_some());
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn routing_read_model_unknown_does_not_infer_direct_from_flags() {
    for missing in [false, true] {
        let fixture = Fixture::claude();
        let path = DeviceStore::for_device().state_path();
        if missing {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::write(&path, b"malformed synthetic state").unwrap();
        }
        let before = read_current(&path).unwrap();
        let view =
            super::current::read_view(&fixture.state.proxy_service, &AppType::Claude).unwrap();
        assert_eq!(view.status, "unknown");
        assert_eq!(view.mode, None);
        assert_eq!(view.attached, None);
        assert!(!view.can_write);
        assert_eq!(read_current(&path).unwrap(), before);
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_read_model() -> Result<(), AppError> {
    routing_read_model_exposes_owner_facts_without_recovery_or_secrets();
    routing_read_model_unknown_does_not_infer_direct_from_flags();
    workspace_metadata_actions_cannot_mutate_through_pending();
    workspace_metadata_actions_preserve_ready_and_legacy_admission();
    workspace_metadata_actions_refuse_unknown_route();
    workspace_metadata_admission_preserves_independent_pi_owner();
    println!("PASS routing owner read model is non-secret, read-only, fail-closed and preserves source17");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn workspace_metadata_actions_cannot_mutate_through_pending() {
    let fixture = Fixture::claude();
    crate::services::order_profiles::save(&fixture.state.db, "claude", "Saved", &["a".into()])
        .unwrap();
    fixture
        .runtime
        .block_on(fixture.state.db.update_provider_health(
            "a",
            "claude",
            false,
            Some("synthetic failure".into()),
        ))
        .unwrap();
    ProviderService::update_sort_order(
        &fixture.state,
        AppType::Claude,
        vec![
            crate::services::provider::ProviderSortUpdate {
                id: "a".into(),
                sort_index: 0,
            },
            crate::services::provider::ProviderSortUpdate {
                id: "b".into(),
                sort_index: 1,
            },
        ],
    )
    .unwrap();
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
    assert!(
        fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .set_failover_for_app("claude", false)
            )
            .is_err(),
        "disabling failover must respect pending admission"
    );
    let before = crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
    let mut created = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    created.id = "new-provider".into();
    let guarded = [
        (
            "sort",
            ProviderService::update_sort_order(
                &fixture.state,
                AppType::Claude,
                vec![crate::services::provider::ProviderSortUpdate {
                    id: "a".into(),
                    sort_index: 99,
                }],
            )
            .is_err(),
        ),
        (
            "add",
            ProviderService::add(&fixture.state, AppType::Claude, created, false).is_err(),
        ),
        (
            "duplicate",
            ProviderService::duplicate(&fixture.state, AppType::Claude, "a").is_err(),
        ),
        (
            "delete",
            ProviderService::delete(&fixture.state, AppType::Claude, "b").is_err(),
        ),
    ];
    assert!(
        guarded.iter().all(|(_, refused)| *refused),
        "provider write admission: {guarded:?}"
    );
    use crate::services::application_selection as selection;
    assert!(selection::save_order_profile(
        &fixture.state,
        &AppType::Claude,
        "Another",
        &["b".into()]
    )
    .is_err());
    assert!(
        selection::rename_order_profile(&fixture.state, &AppType::Claude, "Saved", "Renamed")
            .is_err()
    );
    assert!(selection::remove_order_profile(&fixture.state, &AppType::Claude, "Saved").is_err());
    assert!(selection::import_order_profiles(
        &fixture.state,
        &AppType::Claude,
        vec![crate::services::order_profiles::OrderProfile {
            name: "Incoming".into(),
            provider_ids: vec!["b".into()]
        }]
    )
    .is_err());
    assert!(fixture
        .runtime
        .block_on(selection::set_tier_blocked(
            &fixture.state,
            &AppType::Claude,
            "a",
            true
        ))
        .is_err());
    assert!(fixture
        .runtime
        .block_on(
            fixture
                .state
                .proxy_service
                .reset_routing_errors("a", "claude")
        )
        .is_err());
    assert_eq!(
        crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap(),
        before
    );
    assert!(crate::services::order_profiles::get(&fixture.state.db, "claude").is_ok());
    assert!(crate::services::order_profiles::export_json(&fixture.state.db, "claude").is_ok());
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn workspace_metadata_actions_preserve_ready_and_legacy_admission() {
    for modern in [true, false] {
        let fixture = Fixture::for_app_with_schema(AppType::Claude, modern);
        use crate::services::application_selection as selection;
        selection::save_order_profile(&fixture.state, &AppType::Claude, "Saved", &["a".into()])
            .unwrap();
        selection::rename_order_profile(&fixture.state, &AppType::Claude, "Saved", "Renamed")
            .unwrap();
        selection::remove_order_profile(&fixture.state, &AppType::Claude, "Renamed").unwrap();
        selection::import_order_profiles(
            &fixture.state,
            &AppType::Claude,
            vec![crate::services::order_profiles::OrderProfile {
                name: "Incoming".into(),
                provider_ids: vec!["b".into()],
            }],
        )
        .unwrap();
        fixture
            .runtime
            .block_on(selection::set_tier_blocked(
                &fixture.state,
                &AppType::Claude,
                "b",
                true,
            ))
            .unwrap();
        fixture
            .runtime
            .block_on(
                fixture
                    .state
                    .proxy_service
                    .reset_routing_errors("b", "claude"),
            )
            .unwrap();
        ProviderService::update_sort_order(
            &fixture.state,
            AppType::Claude,
            vec![crate::services::provider::ProviderSortUpdate {
                id: "a".into(),
                sort_index: 99,
            }],
        )
        .unwrap();
        let mut created = fixture
            .state
            .db
            .get_provider_by_id("b", "claude")
            .unwrap()
            .unwrap();
        created.id = "new-provider".into();
        ProviderService::add(&fixture.state, AppType::Claude, created, false).unwrap();
        ProviderService::duplicate(&fixture.state, AppType::Claude, "a").unwrap();
        ProviderService::delete(&fixture.state, AppType::Claude, "new-provider").unwrap();
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn workspace_metadata_actions_refuse_unknown_route() {
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
    state::update(
        &DeviceStore::for_device(),
        &fixture.state.db.secret_session().read().unwrap(),
        |live| {
            live.apps.get_mut("claude").unwrap().proxy_route = None;
            Ok(())
        },
    )
    .unwrap();
    assert!(
        !super::current::read_view(&fixture.state.proxy_service, &AppType::Claude)
            .unwrap()
            .can_write
    );
    let before = crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
    assert!(crate::services::application_selection::save_order_profile(
        &fixture.state,
        &AppType::Claude,
        "Unknown",
        &["a".into()]
    )
    .is_err());
    assert_eq!(
        crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap(),
        before
    );
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop())
        .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn workspace_metadata_admission_preserves_independent_pi_owner() {
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
    let provider = crate::provider::Provider::with_id(
        "synthetic-pi".into(),
        "Synthetic Pi".into(),
        json!({
            "name": "Synthetic Pi", "baseUrl": "https://synthetic.invalid/v1", "apiKey": "synthetic-key", "api": "openai-completions", "models": [{"id":"synthetic-model"}]
        }),
        None,
    );
    ProviderService::add(&fixture.state, AppType::Pi, provider, false).unwrap();
    let duplicate =
        ProviderService::duplicate(&fixture.state, AppType::Pi, "synthetic-pi").unwrap();
    assert_eq!(duplicate.id, "synthetic-pi-copy");
    assert!(
        fixture.pending().is_some(),
        "independent writes must not recover Claude"
    );
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn modern_legacy_common_snippet_writes_are_frozen() {
    let fixture = Fixture::claude();
    fixture
        .state
        .db
        .set_config_snippet("claude", Some("{\"includeCoAuthoredBy\":false}".into()))
        .unwrap();
    let before = crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
    let live = read_current(&fixture.path).unwrap();
    let generic = crate::services::config::ConfigService::set_common_config_snippet(
        &fixture.state,
        "claude",
        "{\"includeCoAuthoredBy\":true}".into(),
    );
    let legacy = crate::services::config::ConfigService::set_claude_common_config_snippet(
        &fixture.state,
        "".into(),
    );
    assert!(
        generic.is_err() && legacy.is_err(),
        "modern snippet writes: general={generic:?}, legacy={legacy:?}"
    );
    assert_eq!(
        crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap(),
        before
    );
    assert_eq!(read_current(&fixture.path).unwrap(), live);
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_legacy_snippet_freeze() -> Result<(), AppError> {
    modern_legacy_common_snippet_writes_are_frozen();
    modern_manual_import_does_not_mutate_frozen_snippet();
    legacy_snippet_freeze_preserves_reads_recovery_and_source17();
    println!("PASS modern legacy snippet mutation is frozen");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_snippet_freeze_preserves_reads_recovery_and_source17() {
    use crate::services::config::ConfigService;
    for app in [AppType::Claude, AppType::Codex, AppType::Gemini] {
        for modern in [false, true] {
            let fixture = Fixture::for_app_with_schema(app.clone(), modern);
            let original = if app == AppType::Codex {
                "[tui]\nnotifications = true\n"
            } else {
                "{\"safe_shared_field\":true}"
            };
            fixture
                .state
                .db
                .set_config_snippet(app.as_str(), Some(original.into()))
                .unwrap();
            if modern {
                {
                    let _fault = Fault::at("pending");
                    assert!(fixture
                        .runtime
                        .block_on(
                            fixture
                                .state
                                .proxy_service
                                .set_takeover_for_app(app.as_str(), true)
                        )
                        .is_err());
                }
                let before =
                    crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap())
                        .unwrap();
                let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
                let live = read_current(&fixture.path).unwrap();
                for value in ["", original] {
                    assert_eq!(
                        ConfigService::set_common_config_snippet(
                            &fixture.state,
                            app.as_str(),
                            value.into()
                        )
                        .unwrap_err(),
                        "mode.legacy_common_config_frozen"
                    );
                }
                let read = super::current::read_view(&fixture.state.proxy_service, &app).unwrap();
                assert!(!read.legacy_common_config_writable);
                assert_eq!(
                    fixture
                        .state
                        .db
                        .get_config_snippet(app.as_str())
                        .unwrap()
                        .as_deref(),
                    Some(original)
                );
                assert_eq!(
                    crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap())
                        .unwrap(),
                    before
                );
                assert_eq!(
                    std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
                    journal
                );
                assert_eq!(read_current(&fixture.path).unwrap(), live);
                super::controller::recover_locked(&fixture.state.proxy_service, &app).unwrap();
                fixture
                    .runtime
                    .block_on(fixture.state.proxy_service.stop_with_restore())
                    .unwrap();
                assert_eq!(
                    fixture
                        .state
                        .db
                        .get_config_snippet(app.as_str())
                        .unwrap()
                        .as_deref(),
                    Some(original)
                );
            } else {
                ConfigService::set_common_config_snippet(
                    &fixture.state,
                    app.as_str(),
                    original.into(),
                )
                .unwrap();
                assert_eq!(
                    fixture
                        .state
                        .db
                        .get_config_snippet(app.as_str())
                        .unwrap()
                        .as_deref(),
                    Some(original)
                );
                ConfigService::set_common_config_snippet(&fixture.state, app.as_str(), "".into())
                    .unwrap();
                assert!(fixture
                    .state
                    .db
                    .get_config_snippet(app.as_str())
                    .unwrap()
                    .is_none());
                if app == AppType::Claude {
                    ConfigService::set_claude_common_config_snippet(
                        &fixture.state,
                        original.into(),
                    )
                    .unwrap();
                    assert_eq!(
                        fixture
                            .state
                            .db
                            .get_config_snippet(app.as_str())
                            .unwrap()
                            .as_deref(),
                        Some(original)
                    );
                }
            }
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn modern_manual_import_does_not_mutate_frozen_snippet() {
    for (modern, existing) in [(true, false), (true, true), (false, false), (false, true)] {
        let fixture = Fixture::for_app_with_schema(AppType::Claude, modern);
        let mut imported = fixture
            .state
            .db
            .get_provider_by_id("a", "claude")
            .unwrap()
            .unwrap();
        imported.settings_config["unowned"] = json!({"keep":true});
        fixture.state.db.save_provider("claude", &imported).unwrap();
        if existing {
            fixture
                .state
                .db
                .set_config_snippet("claude", Some("{\"unowned\":{\"keep\":true}}".into()))
                .unwrap();
        }
        let before =
            crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
        ProviderService::finish_import_common_config(&fixture.state, AppType::Claude).unwrap();
        let after =
            crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
        if modern {
            assert_eq!(
                after, before,
                "manual import snippet side effects, existing={existing}"
            );
        } else {
            assert_ne!(
                after, before,
                "source17 import retains extraction/migration"
            );
            assert!(fixture
                .state
                .db
                .get_config_snippet("claude")
                .unwrap()
                .is_some());
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_manual_alias_respects_pending_admission() {
    let fixture = Fixture::claude();
    fixture
        .state
        .db
        .set_setting("auto_mode_enabled_claude", "true")
        .unwrap();
    fixture
        .state
        .db
        .set_setting("easy_mode_manual_order_claude", "[\"b\",\"a\"]")
        .unwrap();
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
    let before = crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let result = fixture.runtime.block_on(
        crate::services::application_selection::initialize_manual_routing(
            &fixture.state,
            &AppType::Claude,
        ),
    );
    let after = crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
    assert!(
        result.is_err() && after == before,
        "manual alias={result:?}, changed_database={}",
        after != before
    );
    assert_eq!(
        std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
        journal
    );
    fixture
        .runtime
        .block_on(
            crate::services::application_selection::initialize_manual_routing(
                &fixture.state,
                &AppType::Pi,
            ),
        )
        .unwrap();
    assert!(
        fixture.pending().is_some(),
        "independent app migration must not recover Claude"
    );
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_manual_alias_admission() -> Result<(), AppError> {
    legacy_manual_alias_respects_pending_admission();
    legacy_manual_alias_preserves_ready_and_legacy_behavior();
    println!("PASS legacy manual-mode alias respects existing app admission");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_manual_alias_preserves_ready_and_legacy_behavior() {
    use crate::services::application_selection::initialize_manual_routing;
    for modern in [false, true] {
        let fixture = Fixture::for_app_with_schema(AppType::Claude, modern);
        fixture
            .state
            .db
            .set_setting("easy_mode_manual_order_claude", "[\"b\",\"a\"]")
            .unwrap();
        fixture
            .runtime
            .block_on(initialize_manual_routing(&fixture.state, &AppType::Claude))
            .unwrap();
        assert!(fixture
            .state
            .db
            .get_setting("easy_mode_manual_order_claude")
            .unwrap()
            .is_none());
        assert!(fixture
            .state
            .db
            .get_setting("application_priority_claude")
            .unwrap()
            .is_some());
        let before =
            crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
        fixture
            .runtime
            .block_on(initialize_manual_routing(&fixture.state, &AppType::Claude))
            .unwrap();
        assert_eq!(
            crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap(),
            before
        );
        if modern {
            std::fs::write(
                DeviceStore::for_device().state_path(),
                b"malformed synthetic state",
            )
            .unwrap();
            assert!(fixture
                .runtime
                .block_on(initialize_manual_routing(&fixture.state, &AppType::Claude))
                .is_err());
            assert_eq!(
                crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap(),
                before
            );
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn modern_service_construction_does_not_migrate_pending_routing() {
    let fixture = Fixture::claude();
    fixture
        .state
        .db
        .set_setting("auto_mode_enabled_claude", "true")
        .unwrap();
    fixture
        .state
        .db
        .set_setting("easy_mode_manual_order_claude", "[\"b\",\"a\"]")
        .unwrap();
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
    let before = crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let live = read_current(&fixture.path).unwrap();
    let rebuilt = AppState::new(fixture.state.db.clone()).unwrap();
    assert_eq!(
        crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap(),
        before,
        "modern construction must not migrate routing during pending"
    );
    assert_eq!(
        std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
        journal
    );
    assert_eq!(read_current(&fixture.path).unwrap(), live);
    assert!(!fixture.runtime.block_on(rebuilt.proxy_service.is_running()));
    assert!(fixture.pending().is_some());
    drop(rebuilt);
    super::controller::recover_locked(&fixture.state.proxy_service, &AppType::Claude).unwrap();
    fixture
        .runtime
        .block_on(fixture.state.proxy_service.stop_with_restore())
        .unwrap();
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_constructor_admission() -> Result<(), AppError> {
    modern_service_construction_does_not_migrate_pending_routing();
    service_constructor_only_migrates_admitted_legacy_schema();
    println!("PASS modern service construction is passive and source17 retains legacy migration");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn service_constructor_only_migrates_admitted_legacy_schema() {
    for version in [17, 18, 20, 21] {
        let fixture = Fixture::for_app_with_schema(AppType::Claude, version == 20);
        fixture
            .state
            .db
            .conn
            .lock()
            .unwrap()
            .pragma_update(None, "user_version", version)
            .unwrap();
        fixture
            .state
            .db
            .set_setting("auto_mode_enabled_claude", "true")
            .unwrap();
        fixture
            .state
            .db
            .set_setting("easy_mode_manual_order_claude", "[\"b\",\"a\"]")
            .unwrap();
        let before =
            crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        let rebuilt = AppState::new(fixture.state.db.clone()).unwrap();
        let after =
            crate::Database::content_digest(&fixture.state.db.conn.lock().unwrap()).unwrap();
        if version == 17 {
            assert_ne!(after, before);
            assert!(fixture
                .state
                .db
                .get_setting("auto_mode_enabled_claude")
                .unwrap()
                .is_none());
            assert!(fixture
                .state
                .db
                .get_setting("application_priority_claude")
                .unwrap()
                .is_some());
        } else {
            assert_eq!(after, before, "constructor must not write schema {version}");
        }
        assert_eq!(
            std::fs::read(DeviceStore::for_device().state_path()).unwrap(),
            journal
        );
        assert!(!fixture.runtime.block_on(rebuilt.proxy_service.is_running()));
    }
}

fn editor_row_target(previous: &Provider, planned: &Provider) -> state::PendingTarget {
    state::PendingTarget {
        save_request: Some(state::SaveRequest {
            id: "33333333-3333-4333-8333-333333333333".into(),
            provider_id: planned.id.clone(),
            draft_digest: crate::database::Database::provider_update_digest(planned).unwrap(),
            revision: "e".repeat(64),
        }),
        saved_row: Some(state::SavedRow {
            before: crate::database::Database::provider_update_digest(previous).unwrap(),
            provider: crate::database::Database::provider_update_value(planned).unwrap(),
            clear_model_preference: false,
        }),
        ..Default::default()
    }
}

#[test]
#[serial_test::serial]
fn editor_row_save_marks_unchanged_row_in_original_transaction() {
    let fixture = Fixture::claude();
    let row = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    assert!(!fixture.state.db.get_user_edited("claude", "b").unwrap());
    let write =
        super::operation::AppWrite::begin_mode(&fixture.state.proxy_service, &AppType::Claude)
            .unwrap();
    write
        .run(state::op::APPLY, &[], editor_row_target(&row, &row))
        .unwrap();
    assert!(
        fixture.state.db.get_user_edited("claude", "b").unwrap(),
        "explicit save owns the original mark even when row contents are equal"
    );
    assert_eq!(read_current(&fixture.path).unwrap().unwrap(), br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-key-a","ANTHROPIC_BASE_URL":"https://synthetic-a.invalid","ANTHROPIC_MODEL":"synthetic-model-a"},"unowned":{"keep":true}}"#);
}

#[test]
#[serial_test::serial]
fn editor_row_and_maintenance_mark_roll_back_together() {
    let fixture = Fixture::claude();
    let old = fixture
        .state
        .db
        .get_provider_by_id("b", "claude")
        .unwrap()
        .unwrap();
    let mut edited = old.clone();
    edited.name = "synthetic manual edit".into();
    fixture.state.db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_editor_mark BEFORE UPDATE OF user_edited ON providers WHEN NEW.user_edited=1 BEGIN SELECT RAISE(FAIL,'synthetic editor mark failure'); END;").unwrap();
    let write =
        super::operation::AppWrite::begin_mode(&fixture.state.proxy_service, &AppType::Claude)
            .unwrap();
    let result = write.run(state::op::APPLY, &[], editor_row_target(&old, &edited));
    assert!(
        result.is_err(),
        "mark failure must fail the same row transaction"
    );
    let actual = fixture
        .state
        .db
        .get_provider_by_id_with_vault(
            "b",
            "claude",
            fixture.state.db.secret_session(),
            &write.vault,
        )
        .unwrap()
        .unwrap();
    assert_eq!(actual.name, old.name);
    assert!(!fixture.state.db.get_user_edited("claude", "b").unwrap());
    assert!(state::pending(&write.store, &write.vault, "claude")
        .unwrap()
        .is_some());
}

#[test]
#[serial_test::serial]
fn gemini_credential_deletion_preserves_original_save_only_and_active_boundary() {
    for id in ["b", "a"] {
        let fixture = Fixture::for_app(AppType::Gemini);
        let native = read_current(&fixture.path).unwrap();
        let previous = fixture
            .state
            .db
            .get_provider_by_id(id, "gemini")
            .unwrap()
            .unwrap();
        let mut without_key = previous.clone();
        without_key.settings_config["env"]
            .as_object_mut()
            .unwrap()
            .remove("GEMINI_API_KEY");
        let result = ProviderService::update(&fixture.state, AppType::Gemini, None, without_key);
        let after = fixture
            .state
            .db
            .get_provider_by_id(id, "gemini")
            .unwrap()
            .unwrap();
        if id == "b" {
            assert!(
                result.is_ok(),
                "original inactive save permits a structurally valid row without key"
            );
            assert!(after.settings_config["env"].get("GEMINI_API_KEY").is_none());
        } else {
            assert!(
                result.is_err(),
                "active original projector requires the configured API-key credential"
            );
            assert_eq!(after.settings_config, previous.settings_config);
        }
        assert_eq!(
            read_current(&fixture.path).unwrap(),
            native,
            "neither path switches native authentication mode"
        );
        assert!(state::pending(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            "gemini"
        )
        .unwrap()
        .is_none());
    }
}

#[test]
#[serial_test::serial]
fn grok_credential_deletion_preserves_original_required_auth_for_all_save_actions() {
    for id in ["b", "a"] {
        let fixture = Fixture::for_app(AppType::GrokBuild);
        let native = read_current(&fixture.path).unwrap();
        let previous = fixture
            .state
            .db
            .get_provider_by_id(id, "grokbuild")
            .unwrap()
            .unwrap();
        let mut without_key = previous.clone();
        let mut config = without_key.settings_config["config"]
            .as_str()
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        config["model"][id]
            .as_table_like_mut()
            .unwrap()
            .remove("api_key");
        without_key.settings_config["config"] = json!(config.to_string());
        let result = ProviderService::update(&fixture.state, AppType::GrokBuild, None, without_key);
        assert!(
            result.is_err(),
            "original Grok save requires api_key or an explicit env_key even when inactive"
        );
        let after = fixture
            .state
            .db
            .get_provider_by_id(id, "grokbuild")
            .unwrap()
            .unwrap();
        assert_eq!(after.settings_config, previous.settings_config);
        assert_eq!(read_current(&fixture.path).unwrap(), native);
        assert!(state::pending(
            &DeviceStore::for_device(),
            &fixture.state.db.secret_session().read().unwrap(),
            "grokbuild"
        )
        .unwrap()
        .is_none());
    }
}

#[test]
#[serial_test::serial]
fn editor_initial_read_uses_original_mode_and_safe_request_envelope() {
    for app in crate::mode::controller::PROXY_APPS {
        let fixture = Fixture::for_app(app.clone());
        let before = read_current(&fixture.path).unwrap();
        let row = fixture
            .state
            .db
            .get_provider_by_id("b", app.as_str())
            .unwrap()
            .unwrap();
        let value = ProviderService::edit_settings(&fixture.state, app.clone(), "b").unwrap();
        assert!(
            value["settingsConfig"] == row.settings_config,
            "original editor read must return the existing settings in its thin envelope"
        );
        assert_eq!(value["modeState"]["status"], "ready");
        assert!(value
            .get("originalSave")
            .is_none_or(serde_json::Value::is_null));
        let target = editor_row_target(&row, &row);
        let request = target.save_request.clone().unwrap();
        let store = DeviceStore::for_device();
        state::set_pending(
            &store,
            &fixture.state.db.secret_session().read().unwrap(),
            app.as_str(),
            Some(state::Pending {
                op: state::op::APPLY.into(),
                files: vec![],
                target,
                published: false,
                extra: Default::default(),
            }),
        )
        .unwrap();
        let journal = std::fs::read(store.state_path()).unwrap();
        let value = ProviderService::edit_settings(&fixture.state, app.clone(), "b").unwrap();
        assert_eq!(value["originalSave"]["app"], app.as_str());
        assert_eq!(
            value["originalSave"]["request"],
            serde_json::to_value(&request).unwrap()
        );
        assert_eq!(value["originalSave"]["status"], "pending");
        let receipt = serde_json::to_string(&value["originalSave"]).unwrap();
        assert!(!receipt.contains("synthetic-key"));
        assert!(!receipt.contains("settings_config"));
        assert_eq!(std::fs::read(store.state_path()).unwrap(), journal);
        assert_eq!(read_current(&fixture.path).unwrap(), before);

        // A reopened active editor must still be able to query its original
        // operation while its native file is a partial/invalid image.
        let active = fixture
            .state
            .db
            .get_provider_by_id("a", app.as_str())
            .unwrap()
            .unwrap();
        let target = editor_row_target(&active, &active);
        state::set_pending(
            &store,
            &fixture.state.db.secret_session().read().unwrap(),
            app.as_str(),
            Some(state::Pending {
                op: state::op::APPLY.into(),
                files: vec![],
                target,
                published: true,
                extra: Default::default(),
            }),
        )
        .unwrap();
        std::fs::write(&fixture.path, b"synthetic partial native [ invalid").unwrap();
        let journal = std::fs::read(store.state_path()).unwrap();
        let value = ProviderService::edit_settings(&fixture.state, app.clone(), "a").unwrap();
        assert!(
            value["settingsConfig"] == active.settings_config,
            "partial native files cannot replace the original row while querying a pending save"
        );
        assert_eq!(value["originalSave"]["request"]["providerId"], "a");
        assert!(matches!(
            value["originalSave"]["status"].as_str(),
            Some("partial" | "verificationRequired")
        ));
        assert_eq!(std::fs::read(store.state_path()).unwrap(), journal);
        assert_eq!(
            read_current(&fixture.path).unwrap().unwrap(),
            b"synthetic partial native [ invalid"
        );
    }
}

#[test]
#[serial_test::serial]
fn editor_normalization_does_not_resolve_grok_environment_credentials() {
    const NAME: &str = "LOONGPORT_U02_SYNTHETIC_ENV_KEY_73B807E9";
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            std::env::remove_var(NAME);
        }
    }
    assert!(std::env::var_os(NAME).is_none());
    let _reset = Reset;
    let fixture = Fixture::for_app(AppType::GrokBuild);
    let mut row = fixture
        .state
        .db
        .get_provider_by_id("b", "grokbuild")
        .unwrap()
        .unwrap();
    let mut config = row.settings_config["config"]
        .as_str()
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    config["model"]["b"]
        .as_table_like_mut()
        .unwrap()
        .remove("api_key");
    config["model"]["b"]["env_key"] = toml_edit::value(NAME);
    row.settings_config["config"] = json!(config.to_string());
    row.meta.get_or_insert_with(Default::default).usage_script = Some(serde_json::from_value(json!({
        "enabled": false, "language": "javascript", "code": "return {}", "apiKey": "synthetic-env-value"
    })).unwrap());
    std::env::set_var(NAME, "synthetic-env-value");
    let first =
        ProviderService::prepare_provider_update(&fixture.state, &AppType::GrokBuild, row.clone())
            .unwrap();
    std::env::set_var(NAME, "synthetic-env-other");
    let second =
        ProviderService::prepare_provider_update(&fixture.state, &AppType::GrokBuild, row).unwrap();
    assert!(
        serde_json::to_value(first).unwrap() == serde_json::to_value(second).unwrap(),
        "pure editor normalization cannot adopt or compare a process environment credential"
    );
}

#[test]
fn editor_codex_delete_uses_original_inline_table_bearer_owner() {
    let text = "model_provider = \"synthetic\"\nmodel_providers = { synthetic = { name = \"Synthetic\", experimental_bearer_token = \"synthetic-remove\", base_url = \"https://synthetic.invalid\" }, other = { experimental_bearer_token = \"synthetic-keep\" } }\n# untouched\n";
    let after = crate::codex_config::remove_codex_experimental_bearer_token_if(text, |value| {
        value == "synthetic-remove"
    })
    .unwrap();
    let doc = after.parse::<toml_edit::DocumentMut>().unwrap();
    assert!(
        doc["model_providers"]["synthetic"]
            .as_table_like()
            .unwrap()
            .get("experimental_bearer_token")
            .is_none(),
        "explicit removal must reach the original selected inline-table credential"
    );
    assert_eq!(
        doc["model_providers"]["other"]["experimental_bearer_token"].as_str(),
        Some("synthetic-keep")
    );
    assert!(after.ends_with("# untouched\n"));
}

#[test]
#[serial_test::serial]
fn editor_real_preview_confirm_query_closes_original_four_app_save_only() {
    for app in crate::mode::controller::PROXY_APPS {
        let fx = Fixture::for_app(app.clone());
        let mut edited = fx
            .state
            .db
            .get_provider_by_id("b", app.as_str())
            .unwrap()
            .unwrap();
        edited.name = "Synthetic edited name".into();
        let native = read_current(&fx.path).unwrap();
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        let preview = crate::services::provider::edit::preview(
            &fx.state,
            &app,
            edited.clone(),
            "b",
            "d15d3030-1000-4000-8000-000000000001",
            false,
        )
        .unwrap();
        assert_eq!(preview["status"], "ready");
        assert_eq!(preview["action"], "saveOnly");
        assert!(preview["files"].as_array().unwrap().is_empty());
        assert!(!preview.to_string().contains("synthetic-key"));
        assert!(read_current(&fx.path).unwrap() == native);
        assert!(std::fs::read(DeviceStore::for_device().state_path()).unwrap() == journal);
        assert_eq!(
            fx.state
                .db
                .get_provider_by_id("b", app.as_str())
                .unwrap()
                .unwrap()
                .name,
            "b"
        );
        let request: state::SaveRequest =
            serde_json::from_value(preview["request"].clone()).unwrap();
        let result = crate::services::provider::edit::confirm(
            &fx.state,
            &app,
            edited.clone(),
            "b",
            request.clone(),
            false,
        )
        .unwrap();
        assert_eq!(result["status"], "completed");
        assert_eq!(
            fx.state
                .db
                .get_provider_by_id("b", app.as_str())
                .unwrap()
                .unwrap()
                .name,
            edited.name
        );
        assert!(fx.state.db.get_user_edited(app.as_str(), "b").unwrap());
        assert!(read_current(&fx.path).unwrap() == native);
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        let repeated = crate::services::provider::edit::confirm(
            &fx.state,
            &app,
            edited,
            "b",
            request.clone(),
            false,
        )
        .unwrap();
        assert_eq!(repeated["status"], "completed");
        let queried = crate::services::provider::edit::query(&fx.state, &app, request).unwrap();
        assert_eq!(queried["status"], "completed");
        assert!(std::fs::read(DeviceStore::for_device().state_path()).unwrap() == journal);
    }
}

#[test]
#[serial_test::serial]
fn editor_real_claude_preview_conflict_and_original_result_are_read_only() {
    let fx = Fixture::claude();
    let mut edited = fx
        .state
        .db
        .get_provider_by_id("a", "claude")
        .unwrap()
        .unwrap();
    edited.settings_config["env"]["ANTHROPIC_MODEL"] = json!("synthetic-edited-model");
    let preview = crate::services::provider::edit::preview(
        &fx.state,
        &AppType::Claude,
        edited.clone(),
        "a",
        "d15d3030-1000-4000-8000-000000000002",
        false,
    )
    .unwrap();
    assert_eq!(preview["status"], "ready");
    assert_eq!(preview["action"], "saveAndApply");
    assert_eq!(preview["files"][0]["role"], "claudeSettings");
    let request: state::SaveRequest = serde_json::from_value(preview["request"].clone()).unwrap();
    std::fs::write(
        &fx.path,
        br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-external-key"}}"#,
    )
    .unwrap();
    let native = read_current(&fx.path).unwrap();
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    let result = crate::services::provider::edit::confirm(
        &fx.state,
        &AppType::Claude,
        edited,
        "a",
        request.clone(),
        false,
    )
    .unwrap();
    assert_eq!(result["status"], "stale");
    assert!(read_current(&fx.path).unwrap() == native);
    assert!(std::fs::read(DeviceStore::for_device().state_path()).unwrap() == journal);
    let queried =
        crate::services::provider::edit::query(&fx.state, &AppType::Claude, request).unwrap();
    assert_eq!(queried["status"], "notRecorded");
    assert!(!queried.to_string().contains("synthetic-external-key"));
}

#[test]
#[serial_test::serial]
fn editor_real_four_app_direct_save_and_lost_response_query() {
    for app in crate::mode::controller::PROXY_APPS {
        let fx = Fixture::for_app(app.clone());
        let mut edited = fx
            .state
            .db
            .get_provider_by_id("a", app.as_str())
            .unwrap()
            .unwrap();
        match app {
            AppType::Claude => {
                edited.settings_config["env"]["ANTHROPIC_MODEL"] = json!("model-edit")
            }
            AppType::Gemini => {
                edited.settings_config["config"]["model"]["name"] = json!("model-edit")
            }
            AppType::Codex | AppType::GrokBuild => {
                let text = edited.settings_config["config"]
                    .as_str()
                    .unwrap()
                    .replace("model-a", "model-edit");
                edited.settings_config["config"] = json!(text);
            }
            _ => unreachable!(),
        }
        let before = read_current(&fx.path).unwrap();
        let preview = crate::services::provider::edit::preview(
            &fx.state,
            &app,
            edited.clone(),
            "a",
            "d15d3030-1000-4000-8000-000000000003",
            false,
        )
        .unwrap();
        assert_eq!(preview["status"], "ready");
        assert_eq!(preview["action"], "saveAndApply");
        assert!(!preview["files"].as_array().unwrap().is_empty());
        assert!(read_current(&fx.path).unwrap() == before);
        assert!(!preview.to_string().contains("synthetic-key"));
        let request: state::SaveRequest =
            serde_json::from_value(preview["request"].clone()).unwrap();
        assert_eq!(
            crate::services::provider::edit::confirm(
                &fx.state,
                &app,
                edited,
                "a",
                request.clone(),
                false
            )
            .unwrap()["status"],
            "completed"
        );
        let all_files = crate::mode::controller::files(&app)
            .unwrap()
            .into_iter()
            .flat_map(|file| read_current(&file.path).unwrap().unwrap_or_default())
            .collect::<Vec<_>>();
        assert!(String::from_utf8_lossy(&all_files).contains("model-edit"));
        assert_eq!(
            crate::services::provider::edit::query(&fx.state, &app, request).unwrap()["status"],
            "completed"
        );
    }
}

#[test]
#[serial_test::serial]
fn editor_real_blank_preserves_and_explicit_delete_keeps_original_auth_boundary() {
    for active in [false, true] {
        let fx = Fixture::claude();
        let id = if active { "a" } else { "b" };
        let mut edited = fx
            .state
            .db
            .get_provider_by_id(id, "claude")
            .unwrap()
            .unwrap();
        let original = edited.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"].clone();
        edited.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"] = json!("  ");
        let preview = crate::services::provider::edit::preview(
            &fx.state,
            &AppType::Claude,
            edited.clone(),
            id,
            "d15d3030-1000-4000-8000-000000000004",
            false,
        )
        .unwrap();
        assert_eq!(preview["status"], "ready");
        let request: state::SaveRequest =
            serde_json::from_value(preview["request"].clone()).unwrap();
        assert_eq!(
            crate::services::provider::edit::confirm(
                &fx.state,
                &AppType::Claude,
                edited,
                id,
                request,
                false
            )
            .unwrap()["status"],
            "completed"
        );
        assert!(
            fx.state
                .db
                .get_provider_by_id(id, "claude")
                .unwrap()
                .unwrap()
                .settings_config["env"]["ANTHROPIC_AUTH_TOKEN"]
                == original
        );
    }
    for active in [false, true] {
        let fx = Fixture::for_app(AppType::Gemini);
        let id = if active { "a" } else { "b" };
        let edited = fx
            .state
            .db
            .get_provider_by_id(id, "gemini")
            .unwrap()
            .unwrap();
        let native = read_current(&fx.path).unwrap();
        let preview = crate::services::provider::edit::preview(
            &fx.state,
            &AppType::Gemini,
            edited.clone(),
            id,
            "d15d3030-1000-4000-8000-000000000005",
            true,
        )
        .unwrap();
        assert_eq!(preview["status"], if active { "blocked" } else { "ready" });
        if !active {
            let request: state::SaveRequest =
                serde_json::from_value(preview["request"].clone()).unwrap();
            assert_eq!(
                crate::services::provider::edit::confirm(
                    &fx.state,
                    &AppType::Gemini,
                    edited,
                    id,
                    request,
                    true
                )
                .unwrap()["status"],
                "completed"
            );
            assert!(fx
                .state
                .db
                .get_provider_by_id(id, "gemini")
                .unwrap()
                .unwrap()
                .settings_config["env"]
                .get("GEMINI_API_KEY")
                .is_none());
        }
        assert!(read_current(&fx.path).unwrap() == native);
    }
}

#[test]
#[serial_test::serial]
fn editor_codex_preview_checks_both_declared_bearer_slots() {
    let fx = Fixture::for_app(AppType::Codex);
    let mut previous = fx
        .state
        .db
        .get_provider_by_id("b", "codex")
        .unwrap()
        .unwrap();
    previous.settings_config["config"] = json!(format!(
        "{}experimental_bearer_token = \"synthetic-key-b\"\n",
        previous.settings_config["config"].as_str().unwrap()
    ));
    fx.state.db.save_provider("codex", &previous).unwrap();
    let before = crate::Database::content_digest(&fx.state.db.conn.lock().unwrap()).unwrap();
    let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
    for (root_value, delete) in [
        ("\"synthetic-new-key\"", true),
        ("\"***masked***\"", false),
        ("42", false),
        ("\"synthetic-other-key\"", false),
    ] {
        let mut draft = previous.clone();
        draft.settings_config["config"] = json!(format!(
            "experimental_bearer_token = {root_value}\n{}",
            draft.settings_config["config"].as_str().unwrap()
        ));
        let preview = crate::services::provider::edit::preview(
            &fx.state,
            &AppType::Codex,
            draft,
            "b",
            "d15d3030-1000-4000-8000-000000000006",
            delete,
        )
        .unwrap();
        assert_eq!(
            preview["status"], "blocked",
            "declared credential slots must each be validated"
        );
        assert!(!preview.to_string().contains("synthetic-"));
    }
    assert_eq!(
        crate::Database::content_digest(&fx.state.db.conn.lock().unwrap()).unwrap(),
        before
    );
    assert!(std::fs::read(DeviceStore::for_device().state_path()).unwrap() == journal);
    let mut spaced = previous.clone();
    spaced.settings_config["config"] = json!(spaced.settings_config["config"]
        .as_str()
        .unwrap()
        .replace(
            "model_provider = \"synthetic\"",
            "model_provider = \" synthetic \""
        )
        .replace(
            "experimental_bearer_token = \"synthetic-key-b\"",
            "experimental_bearer_token = \"synthetic-replacement\""
        ));
    let preview = crate::services::provider::edit::preview(
        &fx.state,
        &AppType::Codex,
        spaced,
        "b",
        "d15d3030-1000-4000-8000-000000000009",
        true,
    )
    .unwrap();
    assert_eq!(
        preview["status"], "blocked",
        "preparation must use the original owner's normalized selector"
    );
    // Valid duplicate slots preserve blank edits; inactive table bytes stay owned by the draft.
    previous.settings_config["config"] = json!(format!("experimental_bearer_token = \"synthetic-key-b\"\n{}\n[model_providers.inactive]\nexperimental_bearer_token = \"synthetic-inactive-keep\"\n", previous.settings_config["config"].as_str().unwrap()));
    fx.state.db.save_provider("codex", &previous).unwrap();
    let mut draft = previous.clone();
    draft.settings_config["config"] = json!(draft.settings_config["config"]
        .as_str()
        .unwrap()
        .replace("synthetic-key-b", "  "));
    draft.settings_config["auth"]["OPENAI_API_KEY"] = json!("");
    let preview = crate::services::provider::edit::preview(
        &fx.state,
        &AppType::Codex,
        draft.clone(),
        "b",
        "d15d3030-1000-4000-8000-000000000008",
        false,
    )
    .unwrap();
    assert_eq!(preview["status"], "ready");
    let request = serde_json::from_value(preview["request"].clone()).unwrap();
    assert_eq!(
        crate::services::provider::edit::confirm(
            &fx.state,
            &AppType::Codex,
            draft,
            "b",
            request,
            false
        )
        .unwrap()["status"],
        "completed"
    );
    let saved = fx
        .state
        .db
        .get_provider_by_id("b", "codex")
        .unwrap()
        .unwrap();
    let doc = saved.settings_config["config"]
        .as_str()
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert!(doc["experimental_bearer_token"].as_str() == Some("synthetic-key-b"));
    assert!(
        doc["model_providers"]["synthetic"]["experimental_bearer_token"].as_str()
            == Some("synthetic-key-b")
    );
    assert!(
        doc["model_providers"]["inactive"]["experimental_bearer_token"].as_str()
            == Some("synthetic-inactive-keep")
    );
}

#[test]
#[serial_test::serial]
fn editor_query_reuses_original_unknown_publication_result_without_recovery() {
    for active in [false, true] {
        let fx = Fixture::claude();
        let id = if active { "a" } else { "b" };
        let mut draft = fx
            .state
            .db
            .get_provider_by_id(id, "claude")
            .unwrap()
            .unwrap();
        draft.name = "Synthetic interrupted edit".into();
        draft.settings_config["env"]["ANTHROPIC_MODEL"] = json!("synthetic-edited-model");
        let preview = crate::services::provider::edit::preview(
            &fx.state,
            &AppType::Claude,
            draft.clone(),
            id,
            "d15d3030-1000-4000-8000-000000000007",
            false,
        )
        .unwrap();
        assert_eq!(preview["status"], "ready");
        let request: state::SaveRequest =
            serde_json::from_value(preview["request"].clone()).unwrap();
        {
            let _fault = Fault::at(if active { "marked" } else { "target" });
            let result = crate::services::provider::edit::confirm(
                &fx.state,
                &AppType::Claude,
                draft.clone(),
                id,
                request.clone(),
                false,
            )
            .unwrap();
            assert_eq!(result["status"], "verificationRequired");
        }
        let before = crate::Database::content_digest(&fx.state.db.conn.lock().unwrap()).unwrap();
        let journal = std::fs::read(DeviceStore::for_device().state_path()).unwrap();
        let native = read_current(&fx.path).unwrap();
        for repeat in [false, true] {
            let result = if repeat {
                crate::services::provider::edit::confirm(
                    &fx.state,
                    &AppType::Claude,
                    draft.clone(),
                    id,
                    request.clone(),
                    false,
                )
            } else {
                crate::services::provider::edit::query(&fx.state, &AppType::Claude, request.clone())
            }
            .unwrap();
            assert_eq!(result["status"], "verificationRequired");
        }
        assert_eq!(
            crate::Database::content_digest(&fx.state.db.conn.lock().unwrap()).unwrap(),
            before
        );
        assert!(std::fs::read(DeviceStore::for_device().state_path()).unwrap() == journal);
        assert!(read_current(&fx.path).unwrap() == native);
    }
}
