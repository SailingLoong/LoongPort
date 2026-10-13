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
        crate::database::vault::stamp(&conn, &vault)?;
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
