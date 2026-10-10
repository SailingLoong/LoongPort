//! Retained-checkpoint Codex native proof; synthetic files only.
use super::*;
use crate::{app_config::AppType, provider::Provider};
use serde_json::json;

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_native_completion_uses_original_owner_without_writes() {
    let f = Fixture::new();
    publish_resume_fixture(&f);
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    let row = Provider::with_id(
        "synthetic-codex".into(),
        "Synthetic".into(),
        json!({
            "auth":{"OPENAI_API_KEY":"synthetic-key"},
            "config":"model = 'synthetic-model'\nmodel_provider = 'original'\n[model_providers.original]\nname = 'Synthetic'\nbase_url = 'https://synthetic.example.invalid/v1'\nwire_api = 'responses'\n"
        }),
        None,
    );
    db.save_provider("codex", &row).unwrap();
    let mut other = row.clone();
    other.id = "synthetic-other".into();
    other.settings_config["auth"]["OPENAI_API_KEY"] = "synthetic-other-key".into();
    other.settings_config["config"] = row.settings_config["config"]
        .as_str()
        .unwrap()
        .replace("synthetic-model", "synthetic-other-model")
        .into();
    db.save_provider("codex", &other).unwrap();
    db.set_current_provider("codex", &row.id).unwrap();
    drop(db);
    let mut settings = crate::settings::AppSettings {
        current_provider_codex: Some(row.id.clone()),
        ..Default::default()
    };
    f.write_settings(&settings);
    f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","attached":false},"claude":{"mode":"future-mode","opaque":900719925474099312345}}}"#);
    let path = crate::codex_config::get_codex_config_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# user comment\nmodel = 'synthetic-model'\nmodel_provider = 'custom'\n[model_providers.custom]\nname = 'Synthetic'\nbase_url = 'https://synthetic.example.invalid/v1'\nwire_api = 'responses'\nexperimental_bearer_token = 'synthetic-key'\n[ui]\ntheme = 'keep'\n").unwrap();
    let auth_path = crate::codex_config::get_codex_auth_path();
    std::fs::write(&auth_path, br#"{"auth_mode":"chatgpt","tokens":{"account_id":"synthetic-native","access_token":"synthetic-access","refresh_token":"synthetic-refresh"}}"#).unwrap();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let before = snapshot(f.home.path());
    let view = review
        .review_app(&inspected, &token, &AppType::Codex)
        .unwrap();
    assert!(
        view.can_complete_app,
        "complete original Codex owned fields must be provable"
    );
    assert!(!view.can_start_upgrade && !view.can_recover_operation);
    assert_eq!(snapshot(f.home.path()), before);
    // Settings that leave this custom route unchanged still bind the review.
    settings.unify_codex_session_history = false;
    f.write_settings(&settings);
    let changed = snapshot(f.home.path());
    let ununified = review
        .review_app(&inspected, &token, &AppType::Codex)
        .unwrap();
    assert!(ununified.can_complete_app);
    assert_ne!(ununified.revision, view.revision);
    assert_eq!(snapshot(f.home.path()), changed);
    let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let port: i64 = conn
        .query_row(
            "SELECT listen_port FROM proxy_config WHERE app_type='claude'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE proxy_config SET listen_port=?1 WHERE app_type='claude'",
        [port + 1],
    )
    .unwrap();
    let changed = snapshot(f.home.path());
    let endpoint = review
        .review_app(&inspected, &token, &AppType::Codex)
        .unwrap();
    assert!(endpoint.can_complete_app);
    assert_ne!(endpoint.revision, ununified.revision);
    assert_eq!(snapshot(f.home.path()), changed);
    conn.execute(
        "UPDATE proxy_config SET listen_port=?1 WHERE app_type='claude'",
        [port],
    )
    .unwrap();
    drop(conn);
    settings.preserve_codex_official_auth_on_switch = false;
    f.write_settings(&settings);
    let changed = snapshot(f.home.path());
    let misplaced = review
        .review_app(&inspected, &token, &AppType::Codex)
        .unwrap();
    assert!(!misplaced.can_complete_app);
    assert_ne!(misplaced.revision, ununified.revision);
    assert_eq!(snapshot(f.home.path()), changed);
    settings.preserve_codex_official_auth_on_switch = true;
    settings.unify_codex_session_history = true;
    f.write_settings(&settings);
    let session = review.runtime_session(&inspected, &token).unwrap();
    crate::settings::unlock_settings(session.clone()).unwrap();
    let runtime = crate::store::AppState::new(std::sync::Arc::new(
        Database::init_with_secrets(session.clone()).unwrap(),
    ))
    .unwrap();
    let before = snapshot(f.home.path());
    drop(
        crate::mode::operation::AppWrite::begin_mode(&runtime.proxy_service, &AppType::Codex)
            .unwrap(),
    );
    assert!(
        crate::mode::operation::AppWrite::begin_mode(&runtime.proxy_service, &AppType::Claude)
            .is_err()
    );
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let original_config = std::fs::read(&path).unwrap();
    let original_auth = std::fs::read(&auth_path).unwrap();
    for changed_config in [
        String::from_utf8(original_config.clone())
            .unwrap()
            .replace("synthetic-key", "wrong-key"),
        "model_providers = 1".into(),
    ] {
        std::fs::write(&path, changed_config).unwrap();
        let changed = snapshot(f.home.path());
        assert!(
            !review
                .review_app(&inspected, &token, &AppType::Codex)
                .unwrap()
                .can_complete_app
        );
        assert!(crate::mode::operation::AppWrite::begin_mode(
            &runtime.proxy_service,
            &AppType::Codex
        )
        .is_err());
        assert_eq!(snapshot(f.home.path()), changed);
    }
    std::fs::write(&path, &original_config).unwrap();
    // Missing mode is explicitly chosen through the existing no-op journal.
    crate::mode::state::update_app(&f.device, &session.read().unwrap(), "codex", |entry| {
        entry.mode = None;
        Ok(())
    })
    .unwrap();
    let missing = review
        .review_app(&inspected, &token, &AppType::Codex)
        .unwrap();
    assert!(missing.can_choose_mode && !missing.can_complete_app);
    let chosen = review
        .select_mode(
            &inspected,
            &token,
            &AppType::Codex,
            &missing.revision,
            &super::super::UpgradeModeChoice {
                mode: crate::mode::state::Mode::Proxy,
                proxy_route: Some(other.id.clone()),
            },
            &runtime,
        )
        .unwrap();
    assert!(chosen.can_complete_app);
    let mode =
        crate::mode::state::mode_state(&f.device, &session.read().unwrap(), "codex").unwrap();
    assert_eq!(mode.mode, Some(crate::mode::state::Mode::Proxy));
    assert_eq!(mode.proxy_route.as_deref(), Some(other.id.as_str()));
    assert!(!mode.attached);
    assert_eq!(std::fs::read(&path).unwrap(), original_config);
    assert_eq!(std::fs::read(&auth_path).unwrap(), original_auth);
    // The actual original direct writer can now run while this checkpoint stays.
    crate::mode::state::update_app(&f.device, &session.read().unwrap(), "codex", |entry| {
        entry.mode = Some(crate::mode::state::Mode::Direct);
        entry.proxy_route = None;
        Ok(())
    })
    .unwrap();
    crate::services::ProviderService::switch(&runtime, AppType::Codex, &other.id).unwrap();
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Codex)
            .unwrap()
            .as_deref(),
        Some(other.id.as_str())
    );
    assert_eq!(std::fs::read(&auth_path).unwrap(), original_auth);
    let written_config = std::fs::read_to_string(&path).unwrap();
    assert!(
        written_config.contains("synthetic-other-model")
            && written_config.contains("synthetic-other-key")
    );
    assert!(written_config.contains("# user comment") && written_config.contains("theme = 'keep'"));
    assert!(
        review
            .review_app(&inspected, &token, &AppType::Codex)
            .unwrap()
            .can_complete_app
    );
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    assert!(
        crate::mode::operation::AppWrite::begin_mode(&runtime.proxy_service, &AppType::Claude)
            .is_err()
    );
    // A missing direct pointer uses the same explicit keep-files choice while
    // preserving the independent detached Proxy route and original native bytes.
    crate::mode::state::update_app(&f.device, &session.read().unwrap(), "codex", |entry| {
        entry.mode = Some(crate::mode::state::Mode::Proxy);
        entry.proxy_route = Some(row.id.clone());
        Ok(())
    })
    .unwrap();
    runtime
        .db
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE providers SET is_current=0 WHERE app_type='codex'",
            [],
        )
        .unwrap();
    crate::settings::set_current_provider(&AppType::Codex, None).unwrap();
    let missing_pointer = review
        .review_app(&inspected, &token, &AppType::Codex)
        .unwrap();
    assert!(missing_pointer.can_choose_provider && !missing_pointer.can_complete_app);
    assert_eq!(
        missing_pointer
            .keep_files_providers
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        vec![other.id.as_str()]
    );
    let mut changed_settings = crate::settings::get_settings();
    changed_settings.unify_codex_session_history = false;
    crate::settings::update_settings(changed_settings.clone()).unwrap();
    let changed = snapshot(f.home.path());
    assert!(review
        .select_provider(
            &inspected,
            &token,
            &AppType::Codex,
            &missing_pointer.revision,
            &other.id,
            &runtime
        )
        .is_err());
    assert_eq!(snapshot(f.home.path()), changed);
    changed_settings.unify_codex_session_history = true;
    crate::settings::update_settings(changed_settings).unwrap();
    let fresh = review
        .review_app(&inspected, &token, &AppType::Codex)
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let selected = review
        .select_provider(
            &inspected,
            &token,
            &AppType::Codex,
            &fresh.revision,
            &other.id,
            &runtime,
        )
        .unwrap();
    assert!(selected.can_complete_app);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read(&auth_path).unwrap(), original_auth);
    let mode =
        crate::mode::state::mode_state(&f.device, &session.read().unwrap(), "codex").unwrap();
    assert_eq!(mode.mode, Some(crate::mode::state::Mode::Proxy));
    assert_eq!(mode.proxy_route.as_deref(), Some(row.id.as_str()));
    assert!(!mode.attached);
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
}
