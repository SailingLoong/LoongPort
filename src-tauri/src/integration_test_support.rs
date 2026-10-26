//! Narrow constructors for the external integration suites' synthetic fixtures.
//! Ordinary release libraries exclude this module; release integration runs must
//! explicitly enable the existing `test-hooks` feature.
use crate::{AppError, AppState, AppType, Database, MultiAppConfig, SecretSession};
use std::{path::PathBuf, sync::Arc};

fn fixture_root(session: &SecretSession) -> Result<PathBuf, AppError> {
    let home = std::env::var_os("CC_SWITCH_TEST_HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Config("integration_fixture.requires_test_home".into()))?;
    let expected = std::env::temp_dir()
        .canonicalize()
        .map_err(|error| AppError::Config(error.to_string()))?
        .join(format!("cc-switch-test-home-{}", std::process::id()));
    let root = home.join(crate::config::APP_DIR_NAME);
    if home != expected
        || home.canonicalize().ok().as_ref() != Some(&home)
        || crate::config::get_home_dir() != home
        || crate::config::get_app_config_dir() != root
        || session.root() != root
    {
        return Err(AppError::Config(
            "integration_fixture.owner_mismatch".into(),
        ));
    }
    // Reuse the storage owner's ancestor check before the first fixture write.
    if !crate::secrets::files::device_directory_exists(&root)?
        || root.canonicalize().ok().as_ref() != Some(&root)
    {
        return Err(AppError::Config(
            "integration_fixture.owner_mismatch".into(),
        ));
    }
    for name in [
        crate::config::DB_FILE_NAME,
        crate::secrets::owned_file::DEVICE_STATE_FILE,
    ] {
        let path = root.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::io(&path, error)),
            Ok(_) => {
                return Err(AppError::Config(
                    "integration_fixture.requires_fresh_database_and_mode".into(),
                ));
            }
        }
    }
    Ok(root)
}

/// Build a modern Direct fixture through the original encrypted state owner.
/// This cannot set a mode on an existing runtime or select an arbitrary home.
pub fn direct_state(
    session: Arc<SecretSession>,
    config: Option<&MultiAppConfig>,
    apps: &[AppType],
) -> Result<AppState, AppError> {
    fixture_root(&session)?;
    if apps.is_empty() || apps.iter().any(|app| !app.supports_local_proxy()) {
        return Err(AppError::Config(
            "integration_fixture.unsupported_app".into(),
        ));
    }
    let db = Database::init_with_secrets(session)?;
    Database::apply_upstream4_migrations_on_conn(&*db.conn.lock()?)?;
    if let Some(config) = config {
        db.migrate_from_json(config)?;
    }
    if !crate::mode::operation::uses_upstream4_schema(&db)? {
        return Err(AppError::Config(
            "integration_fixture.requires_schema20".into(),
        ));
    }
    {
        let vault = db.secret_session().read()?;
        for app in apps {
            crate::mode::state::update_app(
                &crate::live::engine::DeviceStore::for_device(),
                &vault,
                app.as_str(),
                |entry| {
                    entry.mode = Some(crate::mode::state::Mode::Direct);
                    Ok(())
                },
            )?;
        }
    }
    AppState::new(Arc::new(db))
}

/// Build the actual source17 schema; never downgrade a modern fixture database.
pub fn source17_state(
    session: Arc<SecretSession>,
    config: Option<&MultiAppConfig>,
) -> Result<AppState, AppError> {
    let root = fixture_root(&session)?;
    let path = root.join(crate::config::DB_FILE_NAME);
    let conn = rusqlite::Connection::open(path)?;
    Database::create_tables_on_conn(&conn)?;
    Database::apply_upgrade_source_migrations_on_conn(&conn)?;
    crate::database::loongport_schema::apply(&conn)?;
    {
        let vault = session.read()?;
        crate::database::storage::stamp(&conn, &vault)?;
    }
    session.complete_migration()?;
    let db = Database::from_connection(conn, session);
    assert_eq!(
        Database::get_user_version(&db.conn.lock().unwrap())?,
        crate::database::UPSTREAM4_SOURCE_SCHEMA_VERSION
    );
    if let Some(config) = config {
        db.migrate_from_json(config)?;
    }
    let state = AppState::new(Arc::new(db))?;
    assert_eq!(
        Database::get_user_version(&state.db.conn.lock().unwrap())?,
        crate::database::UPSTREAM4_SOURCE_SCHEMA_VERSION
    );
    Ok(state)
}

/// Execute the original startup chain against isolated synthetic disk fixtures.
#[cfg(feature = "test-hooks")]
pub fn verify_normal_upgrade_startup(case: &str) {
    crate::secrets::startup::verify_normal_upgrade_startup(case);
}

/// Run the isolated real-Wry claim fault suite; no renderer command exposes it.
#[cfg(all(feature = "test-hooks", feature = "gui", target_os = "macos"))]
pub async fn verify_zcode_claim_native(app: tauri::AppHandle) {
    crate::zcode_accounts::claim_runtime::test_support::verify(app).await;
}

/// WorkBuddy synthetic upstream, real encrypted owner and authorization consumers.
#[cfg(feature = "gui")]
pub async fn verify_workbuddy_shared_authorization(case: &str) {
    crate::workbuddy::integration_fixture::verify(case).await;
}

/// MiniMax synthetic upstream, real shared account refresh owner.
#[cfg(feature = "gui")]
pub async fn verify_minimax_refresh_owner(case: &str) {
    crate::minimax_code::refresh_owner_fixture::verify(case).await;
}

/// Run the same real SQLite owner case without code-generating unrelated unit suites.
#[cfg(feature = "test-hooks")]
pub fn verify_credential_storage_case(case: &str) -> Result<(), String> {
    crate::database::verify_storage_case(case);
    Ok(())
}

/// Same original session owner cases, no real OS store.
#[cfg(feature = "test-hooks")]
pub fn verify_credential_startup_case(case: &str) {
    crate::secrets::session::protection_startup_tests::verify(case);
}
/// Actual ordinary storage owners. Async groups retain their original runtime.
#[cfg(feature = "test-hooks")]
pub async fn verify_credential_files_case(case: &str) {
    match case {
        "owned" => crate::secrets::owned_file::storage_mode_tests::run(),
        "files" => crate::secrets::files::plaintext_runtime_tests::run(),
        "settings" => crate::settings::plaintext_storage_tests::run(),
        "live" => crate::live::engine::plaintext_tests::run(),
        "workbuddy" => crate::workbuddy::verify_plaintext_storage(),
        "minimax" => crate::minimax_code::catalog::plaintext_tests::run().await,
        "trae" => crate::trae_accounts::store::plaintext_tests::run().await,
        "zcode_checkpoint" => crate::zcode_accounts::checkpoint::plaintext_tests::run(),
        "zcode_key_intent" => crate::zcode_accounts::key_intent::tests::run_plaintext(),
        "zcode_operation_log" => crate::zcode_accounts::operation_log::tests::run_plaintext(),
        _ => panic!("unknown credential file test case"),
    }
}

#[cfg(feature = "test-hooks")]
pub fn verify_credential_transcode_case(case: &str) {
    if case.starts_with("settings_") {
        crate::settings::explicit_storage_transcode_tests::run_case(case);
    } else {
        crate::secrets::files::explicit_storage_inventory_tests::run_case(case);
    }
}

#[cfg(feature = "test-hooks")]
pub fn verify_credential_generation_case(case: &str) {
    crate::database::verify_storage_generation_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_coordinator_protection_entry(case: &str) {
    crate::secrets::startup::protection_entry_tests::run_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_legacy_generation_fence_case(case: &str) {
    crate::secrets::transition::verify_legacy_generation_fence_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_storage_backup_generation_case(case: &str) {
    crate::database::verify_storage_backup_generation_case(case);
}
#[cfg(feature = "test-hooks")]
pub fn verify_generation_files_case(case: &str) {
    crate::secrets::protection::generation_files::tests::run_case(case);
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub fn verify_protection_ipc_case(case: &str) {
    crate::secrets::startup::credential_protection::tests::run_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_legacy_reset_fence_case(case: &str) {
    crate::secrets::reset::verify_legacy_reset_fence_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_native_generation_case(case: &str) {
    crate::secrets::protection::generation::tests::run_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_text_archive_case(case: &str) {
    crate::services::sync_protocol::text_archive_tests::run_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_legacy_reset_cleanup_case(case: &str) {
    crate::secrets::reset::verify_legacy_reset_cleanup_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_backup_image_case(case: &str) {
    crate::database::storage::verify_backup_image_case(case);
}
#[cfg(feature = "test-hooks")]
pub fn verify_generation_image_case(case: &str) {
    crate::database::storage::verify_generation_image_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_local_sql_restore_case(case: &str) {
    crate::services::backup_transfer::storage_restore_tests::run_case(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_imagegen_storage_case(case: &str) {
    crate::imagegen_storage_tests::run(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_file_postflight_case(case: &str) {
    crate::secrets::protection::generation_files::postflight_tests::run_case(case);
}

#[cfg(feature = "test-hooks")]
pub fn verify_joint_storage_content_case(case: &str) {
    crate::secrets::transition::content_tests::run(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_aggregate_storage_export_case(case: &str) {
    crate::secrets::export::storage_tests::run_case(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_sync_admission_case(case: &str) {
    crate::services::sync_protocol::verify_sync_admission_case(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_explicit_legacy_plaintext_file_restore() {
    crate::services::backup_transfer::verify_explicit_legacy_plaintext_file_restore();
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_modern_unmarked_plaintext_backup() {
    crate::services::backup_transfer::verify_modern_unmarked_plaintext_backup();
}

/// Match Tauri's own platform-specific test WebView origin; never changes application ACLs.
#[cfg(all(feature = "gui", any(test, feature = "test-hooks")))]
pub(crate) fn tauri_test_origin() -> url::Url {
    if cfg!(any(windows, target_os = "android")) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
    .parse()
    .expect("Tauri test origin is a static valid URL")
}

#[cfg(feature = "test-hooks")]
pub fn verify_remote_plaintext_case(case: &str) {
    crate::secrets::transition::content_tests::remote_plaintext_tests::run(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_remote_plaintext_transport_case(case: &str) {
    crate::secrets::transition::content_tests::remote_plaintext_transport::run_case(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_original_publication_control(case: &str) {
    crate::services::sync_protocol::verify_original_publication_control(case);
}

#[doc(hidden)]
#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub fn verify_remote_plaintext_recovery_case(case: &str) {
    crate::secrets::transition::content_tests::remote_plaintext_recovery::run(case);
}

#[doc(hidden)]
#[cfg(feature = "test-hooks")]
pub fn verify_archive_stage_privacy_case(case: &str) {
    crate::secrets::transition::content_tests::remote_plaintext_transport::run_archive_case(case);
}

#[doc(hidden)]
#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub fn verify_remote_plaintext_auto_attempt(case: &str) {
    crate::secrets::transition::content_tests::remote_plaintext_transport::run_auto_attempt_case(
        case,
    );
}
