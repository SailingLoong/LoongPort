use super::*;
use crate::database::{self, Database};
use crate::live::engine::DeviceStore;
use crate::secrets::testing::{MemoryKeyStore, TestHome};
use std::sync::Arc;

fn source17(store: &MemoryKeyStore) -> Arc<SecretSession> {
    source17_with_password(store, None)
}

fn source17_with_password(store: &MemoryKeyStore, password: Option<&str>) -> Arc<SecretSession> {
    let root = crate::config::get_app_config_dir();
    let session = SecretSession::open(&root, store, password).unwrap();
    let conn = rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
    Database::create_tables_on_conn(&conn).unwrap();
    Database::apply_upgrade_source_migrations_on_conn(&conn).unwrap();
    database::loongport_schema::apply(&conn).unwrap();
    database::vault::stamp(&conn, session.read().unwrap().encrypted().unwrap()).unwrap();
    session.complete_migration().unwrap();
    let db = Database::from_connection(conn, session.clone());
    db.save_provider("claude", &crate::Provider::with_id(
        "preserved".into(), "Existing provider".into(),
        serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-upgrade-secret","ANTHROPIC_MODEL":"existing-model"}}), None,
    )).unwrap();
    db.set_current_provider("claude", "preserved").unwrap();
    assert_eq!(
        Database::get_user_version(&db.conn.lock().unwrap()).unwrap(),
        17
    );
    drop(db);
    session
}

fn coordinator() -> StartupCoordinator {
    let root = crate::config::get_app_config_dir();
    let inspection = crate::secrets::upgrade::inspect(&root, &DeviceStore::for_device()).unwrap();
    StartupCoordinator::new(root, inspection)
}

fn start(
    coordinator: &StartupCoordinator,
    store: &MemoryKeyStore,
) -> Result<crate::AppState, String> {
    let mut state = None;
    coordinator.initialize_with(None, store, |session| {
        let (runtime, _, _) =
            crate::initialize_runtime_core(session).map_err(|error| error.to_string())?;
        state = Some(runtime);
        Ok(())
    })?;
    Ok(state.expect("first startup must publish the real runtime"))
}

fn assert_usable(state: &crate::AppState, preserved: bool) {
    assert_eq!(
        Database::get_user_version(&state.db.conn.lock().unwrap()).unwrap(),
        17
    );
    assert!(!crate::mode::operation::uses_upstream4_schema(&state.db).unwrap());
    state.db.get_all_providers("claude").unwrap();
    state.db.get_log_config().unwrap();
    for app in crate::mode::controller::PROXY_APPS {
        if let Some(view) = crate::mode::current::read_view(&state.proxy_service, &app) {
            assert!(
                view.can_write,
                "{app:?} remains blocked after successful startup: {}",
                view.status
            );
        }
    }
    assert!(
        crate::mode::current::read_view(&state.proxy_service, &crate::AppType::Claude).is_none()
    );
    if preserved {
        let provider = state
            .db
            .get_provider_by_id("preserved", "claude")
            .unwrap()
            .unwrap();
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"],
            "synthetic-upgrade-secret"
        );
        assert_eq!(
            provider.settings_config["env"]["ANTHROPIC_MODEL"],
            "existing-model"
        );
        assert_eq!(
            state.db.get_current_provider("claude").unwrap().as_deref(),
            Some("preserved")
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
pub(super) fn source17_without_checkpoint_starts_and_preserves_data() {
    let _home = TestHome::new().unwrap();
    let store = MemoryKeyStore::default();
    drop(source17(&store));
    let root = crate::config::get_app_config_dir();
    assert!(!DeviceStore::for_device()
        .root()
        .join(crate::secrets::upgrade::checkpoint::FILE)
        .exists());
    let coordinator = coordinator();
    let state = start(&coordinator, &store)
        .expect("normal existing schema17 must start without a manual upgrade checkpoint");
    assert_usable(&state, true);
    assert!(*coordinator.phase.lock().unwrap() == Phase::Ready);
    coordinator
        .initialize_with(None, &store, |_| panic!("must not initialize twice"))
        .unwrap();
    crate::ProviderService::switch(&state, crate::AppType::Claude, "preserved").unwrap();
    let live = crate::ProviderService::read_live_settings(crate::AppType::Claude).unwrap();
    assert_eq!(
        live["env"]["ANTHROPIC_AUTH_TOKEN"],
        "synthetic-upgrade-secret"
    );
    assert_eq!(live["env"]["ANTHROPIC_MODEL"], "existing-model");
    let edit =
        crate::ProviderService::edit_settings(&state, crate::AppType::Claude, "preserved").unwrap();
    assert!(edit["modeState"].is_null());
    assert_eq!(
        edit["settingsConfig"]["env"]["ANTHROPIC_MODEL"],
        "existing-model"
    );
    let status = crate::rt::block_on(state.proxy_service.get_takeover_status()).unwrap();
    assert!(!status.claude);
    let mut provider = state
        .db
        .get_provider_by_id("preserved", "claude")
        .unwrap()
        .unwrap();
    provider.name = "Edited after normal update".into();
    crate::ProviderService::update(&state, crate::AppType::Claude, Some("preserved"), provider)
        .unwrap();
    assert_eq!(
        state
            .db
            .get_provider_by_id("preserved", "claude")
            .unwrap()
            .unwrap()
            .name,
        "Edited after normal update"
    );
    assert!(!root.join(".vault-migration.db").exists());
    drop(state);
    let _new_process_settings = crate::settings::TestSettingsScope::enter().unwrap();
    let restarted = self::coordinator();
    assert_usable(&start(&restarted, &store).unwrap(), true);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
pub(super) fn fresh_install_starts_without_upgrade_review() {
    let _home = TestHome::new().unwrap();
    let store = MemoryKeyStore::default();
    let coordinator = coordinator();
    let state = start(&coordinator, &store).unwrap();
    assert_usable(&state, false);
    assert!(*coordinator.phase.lock().unwrap() == Phase::Ready);
    let provider = crate::Provider::with_id(
        "new".into(),
        "New install".into(),
        serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-new-secret"}}),
        None,
    );
    crate::ProviderService::add(&state, crate::AppType::Claude, provider, false).unwrap();
    crate::ProviderService::switch(&state, crate::AppType::Claude, "new").unwrap();
    assert_eq!(
        crate::ProviderService::read_live_settings(crate::AppType::Claude).unwrap()["env"]
            ["ANTHROPIC_AUTH_TOKEN"],
        "synthetic-new-secret"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
pub(super) fn failed_database_publication_keeps_recovery_and_restarts() {
    let _home = TestHome::new().unwrap();
    let store = MemoryKeyStore::default();
    let session = source17(&store);
    let root = crate::config::get_app_config_dir();
    // Reproduce a previous interrupted publication using the original stage owner.
    let source_path = root.join(crate::config::DB_FILE_NAME);
    let stage = root.join(".vault-migration.db");
    std::fs::copy(&source_path, &stage).unwrap();
    let original_stage =
        rusqlite::Connection::open_with_flags(&stage, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    database::vault::check_identity(
        &original_stage,
        session.read().unwrap().encrypted().unwrap(),
    )
    .unwrap();
    crate::secrets::inventory::validate_database(
        &original_stage,
        session.read().unwrap().encrypted().unwrap(),
    )
    .unwrap();
    drop(original_stage);
    let valid_stage = std::fs::read(&stage).unwrap();
    let staged = rusqlite::Connection::open(&stage).unwrap();
    staged
        .execute(
            "UPDATE providers SET settings_config='synthetic tamper' WHERE id='preserved'",
            [],
        )
        .unwrap();
    drop(staged);
    let source_before = std::fs::read(&source_path).unwrap();
    let stage_before = std::fs::read(&stage).unwrap();
    let coordinator = coordinator();
    let error = start(&coordinator, &store)
        .err()
        .expect("publication must fail");
    assert!(!error.contains("synthetic-upgrade-secret"));
    assert!(*coordinator.phase.lock().unwrap() == Phase::Failed);
    assert_eq!(std::fs::read(&source_path).unwrap(), source_before);
    assert_eq!(std::fs::read(&stage).unwrap(), stage_before);
    assert!(coordinator
        .initialize_with(None, &store, |_| panic!(
            "failed startup cannot repeat side effects"
        ))
        .is_err());
    std::fs::write(&stage, valid_stage).unwrap();
    let _new_process_settings = crate::settings::TestSettingsScope::enter().unwrap();
    let restarted = self::coordinator();
    assert_usable(&start(&restarted, &store).unwrap(), true);
    assert!(!stage.exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
pub(super) fn password_and_existing_modern_database_keep_original_owners() {
    let _home = TestHome::new().unwrap();
    let store = MemoryKeyStore::default();
    let session = source17_with_password(&store, Some("synthetic-upgrade-password"));
    let root = crate::config::get_app_config_dir();
    let coordinator = coordinator();
    assert_eq!(
        coordinator.initialize_with(None, &store, |_| panic!("locked")),
        Err("secret.locked".into())
    );
    assert!(coordinator
        .initialize_with(Some("wrong synthetic password"), &store, |_| panic!(
            "wrong password"
        ))
        .is_err());
    let mut state = None;
    coordinator
        .initialize_with(Some("synthetic-upgrade-password"), &store, |session| {
            state = Some(
                crate::initialize_runtime_core(session)
                    .map_err(|e| e.to_string())?
                    .0,
            );
            Ok(())
        })
        .unwrap();
    let state = state.unwrap();
    assert_usable(&state, true);
    {
        let conn = state.db.conn.lock().unwrap();
        Database::apply_upstream4_migrations_on_conn(&conn).unwrap();
    }
    let device = DeviceStore::for_device();
    crate::mode::state::update_app(&device, &session.read().unwrap(), "claude", |entry| {
        entry.mode = Some(crate::mode::state::Mode::Direct);
        Ok(())
    })
    .unwrap();
    let mode_before = std::fs::read(device.state_path()).unwrap();
    drop(state);
    let _new_process_settings = crate::settings::TestSettingsScope::enter().unwrap();
    let restarted = self::coordinator();
    let mut modern = None;
    restarted
        .initialize_with(Some("synthetic-upgrade-password"), &store, |session| {
            modern = Some(
                crate::initialize_runtime_core(session)
                    .map_err(|e| e.to_string())?
                    .0,
            );
            Ok(())
        })
        .unwrap();
    let modern = modern.unwrap();
    assert_eq!(
        Database::get_user_version(&modern.db.conn.lock().unwrap()).unwrap(),
        20
    );
    assert!(crate::mode::operation::uses_upstream4_schema(&modern.db).unwrap());
    assert!(
        crate::mode::current::read_view(&modern.proxy_service, &crate::AppType::Claude)
            .unwrap()
            .can_write
    );
    assert_eq!(std::fs::read(device.state_path()).unwrap(), mode_before);
    assert!(!root.join(".vault-migration.db").exists());
}
