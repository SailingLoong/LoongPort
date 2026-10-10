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

fn managed_native_runtime(
    f: &Fixture,
) -> (
    UpgradeInspection,
    AuthenticatedUpgrade,
    String,
    crate::store::AppState,
) {
    let app = AppType::Codex;
    let (inspected, review, token, runtime) = super::native_recovery_tests::native_runtime(f, &app);
    f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","attached":false},"claude":{"mode":"direct","attached":false}}}"#);
    let account = "synthetic-completion-account";
    crate::rt::block_on(
        runtime
            .codex_oauth_manager
            .add_test_account_with_access_token(
                account,
                "synthetic-completion-access",
                Some("synthetic-completion-id"),
            ),
    )
    .unwrap();
    let mut row = Provider::with_id(
        "managed".into(),
        "Synthetic managed".into(),
        json!({"auth":{}, "config":"model = 'gpt-5.5'\n"}),
        None,
    );
    row.category = Some("official".into());
    row.meta = Some(crate::provider::ProviderMeta {
        auth_binding: Some(crate::provider::AuthBinding {
            source: crate::provider::AuthBindingSource::ManagedAccount,
            auth_provider: Some("codex_oauth".into()),
            account_id: Some(account.into()),
        }),
        ..Default::default()
    });
    runtime.db.save_provider("codex", &row).unwrap();
    crate::services::ProviderService::switch(&runtime, app.clone(), "managed").unwrap();
    (inspected, review, token, runtime)
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_admits_original_writer() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let app = AppType::Codex;
    let before = snapshot(f.home.path());
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    assert!(
        !review
            .review_app(&inspected, &token, &app)
            .unwrap()
            .can_complete_app,
        "byte-only review cannot infer a live manager generation"
    );
    assert!(
        review
            .review_app_with_state(&inspected, &token, &app, &runtime)
            .unwrap()
            .can_complete_app
    );
    assert_eq!(snapshot(f.home.path()), before);
    let result = crate::services::ProviderService::switch(&runtime, app, "b");
    assert!(
        result.is_ok(),
        "a completed original managed owner must admit its next native operation: {result:?}"
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Codex)
            .unwrap()
            .as_deref(),
        Some("b")
    );
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    assert_outgoing_managed_placement_is_still_unproven(&f, &inspected, &review, &token, &runtime);
}

// The existing switch out of managed login writes the third-party row auth to
// auth.json while clearing its managed marker. Under preserve=true its next
// self-projection prefers bearer-TOML. Completion must not waive that mismatch.
fn assert_outgoing_managed_placement_is_still_unproven(
    f: &Fixture,
    inspected: &UpgradeInspection,
    review: &AuthenticatedUpgrade,
    token: &str,
    runtime: &crate::store::AppState,
) {
    let row = runtime.db.get_all_providers("codex").unwrap()["b"].clone();
    let auth: serde_json::Value =
        serde_json::from_slice(&std::fs::read(crate::codex_config::get_codex_auth_path()).unwrap())
            .unwrap();
    assert!(
        auth == row.settings_config["auth"],
        "the original outgoing placement is retained"
    );
    let doc: toml::Table = std::fs::read_to_string(crate::codex_config::get_codex_config_path())
        .unwrap()
        .parse()
        .unwrap();
    let route = doc["model_provider"].as_str().unwrap();
    assert_eq!(
        doc["model_providers"][route]["requires_openai_auth"].as_bool(),
        Some(true)
    );
    assert!(doc["model_providers"][route]
        .get("experimental_bearer_token")
        .is_none());
    let before = snapshot(f.home.path());
    let view = review
        .review_app_with_state(inspected, token, &AppType::Codex, runtime)
        .unwrap();
    assert_eq!(view.has_pending_operation, Some(false));
    assert!(
        !view.can_complete_app,
        "marker-sensitive original placement remains unproven"
    );
    assert!(
        crate::mode::operation::AppWrite::begin_mode(&runtime.proxy_service, &AppType::Codex)
            .is_err()
    );
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_requires_persisted_owner() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let file = crate::secrets::files::CredentialFile::Codex;
    let path = file.path(runtime.db.secret_session());
    let original = std::fs::read(&path).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&file.decode(&f.vault, &original).unwrap()).unwrap();
    let mut future = value.clone();
    future["version"] = json!(99);
    let mut deleted = value.clone();
    deleted["accounts"] = json!({});
    let mut replaced = value.clone();
    replaced["accounts"]["synthetic-completion-account"]["refresh_token"] =
        json!("synthetic-disk-newer");
    let mut wrong_id = value.clone();
    wrong_id["accounts"]["synthetic-completion-account"]["account_id"] = json!("synthetic-other");
    let variants = [None, Some(b"invalid-envelope".to_vec())]
        .into_iter()
        .chain([future, deleted, replaced, wrong_id].into_iter().map(|v| {
            Some(
                file.encode(&f.vault, &serde_json::to_vec(&v).unwrap())
                    .unwrap(),
            )
        }));
    for (index, bytes) in variants.enumerate() {
        match bytes {
            Some(bytes) => std::fs::write(&path, bytes).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        let before = snapshot(f.home.path());
        let result = review.review_app_with_state(&inspected, &token, &AppType::Codex, &runtime);
        assert!(
            result.is_err() || !result.unwrap().can_complete_app,
            "unproved persisted manager input must remain blocked: case {index}"
        );
        assert_eq!(
            snapshot(f.home.path()),
            before,
            "review mutated case {index}"
        );
        let result = crate::services::ProviderService::switch(&runtime, AppType::Codex, "b");
        assert!(
            result.is_err(),
            "native writer admitted unproved persisted manager input: {index}"
        );
        assert_eq!(
            snapshot(f.home.path()),
            before,
            "blocked native writer mutated case {index}"
        );
        std::fs::write(&path, &original).unwrap();
    }
}

fn assert_managed_blocked(
    f: &Fixture,
    inspected: &UpgradeInspection,
    review: &AuthenticatedUpgrade,
    token: &str,
    runtime: &crate::store::AppState,
    label: &str,
) {
    let before = snapshot(f.home.path());
    let result = review.review_app_with_state(inspected, token, &AppType::Codex, runtime);
    assert!(
        result.is_err() || !result.unwrap().can_complete_app,
        "unproved native state: {label}"
    );
    assert_eq!(snapshot(f.home.path()), before, "review wrote: {label}");
    assert!(
        crate::services::ProviderService::switch(runtime, AppType::Codex, "b").is_err(),
        "writer admitted: {label}"
    );
    assert_eq!(
        snapshot(f.home.path()),
        before,
        "refused writer wrote: {label}"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_binds_auth_marker_written_and_store_mode() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let files = crate::services::provider::codex_direct::files();
    let auth = std::fs::read(&files[0].path).unwrap();
    let config = std::fs::read_to_string(&files[1].path).unwrap();
    let marker = std::fs::read(&files[3].path).unwrap();
    let auth_value: serde_json::Value = serde_json::from_slice(&auth).unwrap();
    for field in ["account_id", "access_token", "refresh_token", "id_token"] {
        let mut changed = auth_value.clone();
        changed["tokens"][field] = json!("synthetic-unproved");
        std::fs::write(&files[0].path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_managed_blocked(&f, &inspected, &review, &token, &runtime, field);
    }
    for time in [
        json!("2099-01-01T00:00:00Z"),
        json!("2000-01-01T00:00:00Z"),
        json!(null),
    ] {
        let mut changed = auth_value.clone();
        changed["last_refresh"] = time;
        std::fs::write(&files[0].path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_managed_blocked(&f, &inspected, &review, &token, &runtime, "unproved time");
    }
    std::fs::write(&files[0].path, &auth).unwrap();
    for bytes in [
        None,
        Some(br#"{"version":99,"account_id":"synthetic-completion-account"}"#.to_vec()),
        Some(br#"{"version":2,"account_id":"synthetic-other"}"#.to_vec()),
        Some(
            br#"{"version":2,"account_id":"synthetic-completion-account","future":true}"#.to_vec(),
        ),
    ] {
        match bytes {
            Some(bytes) => std::fs::write(&files[3].path, bytes).unwrap(),
            None => std::fs::remove_file(&files[3].path).unwrap(),
        }
        assert_managed_blocked(&f, &inspected, &review, &token, &runtime, "marker");
        std::fs::write(&files[3].path, &marker).unwrap();
    }
    let written = crate::mode::state::written(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "codex",
    )
    .unwrap()
    .unwrap();
    let mut cases = vec![None];
    for field in ["account_id", "last_refresh_ms", "digest"] {
        let mut changed = written.clone();
        let auth = changed.codex.as_mut().unwrap().auth.as_mut().unwrap();
        match field {
            "account_id" => auth.account_id = "synthetic-other".into(),
            "last_refresh_ms" => auth.last_refresh_ms += 1,
            _ => auth.digest = "0".repeat(64),
        }
        cases.push(Some(changed));
    }
    let mut foreign = written.clone();
    foreign.tables.push("synthetic-foreign-grok".into());
    cases.push(Some(foreign));
    let live =
        crate::mode::state::load(&f.device, &runtime.db.secret_session().read().unwrap()).unwrap();
    for changed in cases {
        let mut changed_live = live.clone();
        changed_live.apps.get_mut("codex").unwrap().written = changed;
        f.write_raw_mode(&serde_json::to_vec(&changed_live).unwrap());
        assert_managed_blocked(&f, &inspected, &review, &token, &runtime, "Written");
    }
    f.write_raw_mode(&serde_json::to_vec(&live).unwrap());
    for changed in [
        format!("cli_auth_credentials_store = 'keyring'\n{config}"),
        format!("cli_auth_credentials_store = 'auto'\n{config}"),
        format!("cli_auth_credentials_store = 'ephemeral'\n{config}"),
        config.replace("gpt-5.5", "synthetic-other-model"),
        format!("model_catalog_json = 'loongport-model-catalog.json'\n{config}"),
    ] {
        std::fs::write(&files[1].path, changed).unwrap();
        assert_managed_blocked(
            &f,
            &inspected,
            &review,
            &token,
            &runtime,
            "config/store/catalog",
        );
    }
    std::fs::write(&files[1].path, config).unwrap();
    assert!(
        review
            .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
            .unwrap()
            .can_complete_app
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_rechecks_memory_and_restarted_owner() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let account = "synthetic-completion-account";
    let first = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    assert!(first.can_complete_app);
    // Exact persisted original result survives loss of the unpersisted access cache.
    let db = runtime.db.clone();
    drop(runtime);
    let runtime = crate::store::AppState::new(db).unwrap();
    let before = snapshot(f.home.path());
    let restarted = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    assert!(restarted.can_complete_app);
    assert_ne!(
        restarted.revision, first.revision,
        "cache identity belongs to the reviewed generation"
    );
    assert_eq!(snapshot(f.home.path()), before);
    let file = crate::secrets::files::CredentialFile::Codex;
    let path = file.path(runtime.db.secret_session());
    let bytes = std::fs::read(&path).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&file.decode(&f.vault, &bytes).unwrap()).unwrap();
    let old = value["accounts"][account]["token_updated_at_ms"]
        .as_i64()
        .unwrap();
    // The original manager can be newer than its still-old persistence image.
    crate::rt::block_on(
        runtime
            .codex_oauth_manager
            .test_set_token_updated_at_ms(account, old + 1),
    );
    assert_managed_blocked(
        &f,
        &inspected,
        &review,
        &token,
        &runtime,
        "newer memory, old persistence",
    );
    let changed = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    assert_ne!(changed.revision, restarted.revision);
    crate::rt::block_on(
        runtime
            .codex_oauth_manager
            .test_set_token_updated_at_ms(account, old),
    );
    let fresh = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    assert!(fresh.can_complete_app);
    // Re-encryption changes the bound file even though its semantic account is identical.
    file.write(
        runtime.db.secret_session(),
        &serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let resealed = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    assert!(resealed.can_complete_app);
    assert_ne!(resealed.revision, fresh.revision);
    crate::services::ProviderService::switch(&runtime, AppType::Codex, "b").unwrap();
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_uses_original_keep_files_choices() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let app = AppType::Codex;
    let native = crate::services::provider::codex_direct::files()
        .into_iter()
        .map(|file| (file.path.clone(), std::fs::read(file.path).ok()))
        .collect::<Vec<_>>();
    crate::mode::state::update_app(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "codex",
        |entry| {
            entry.mode = None;
            Ok(())
        },
    )
    .unwrap();
    let missing = review
        .review_app_with_state(&inspected, &token, &app, &runtime)
        .unwrap();
    assert!(missing.can_choose_mode && !missing.can_complete_app);
    let file = crate::secrets::files::CredentialFile::Codex;
    let bytes = file.read(runtime.db.secret_session()).unwrap().unwrap();
    file.write(runtime.db.secret_session(), &bytes).unwrap();
    let before = snapshot(f.home.path());
    let choice = super::super::UpgradeModeChoice {
        mode: crate::mode::state::Mode::Proxy,
        proxy_route: Some("b".into()),
    };
    assert!(review
        .select_mode(
            &inspected,
            &token,
            &app,
            &missing.revision,
            &choice,
            &runtime
        )
        .is_err());
    assert_eq!(
        snapshot(f.home.path()),
        before,
        "stale owner image must not start a choice operation"
    );
    let fresh = review
        .review_app_with_state(&inspected, &token, &app, &runtime)
        .unwrap();
    let selected = review
        .select_mode(&inspected, &token, &app, &fresh.revision, &choice, &runtime)
        .unwrap();
    assert!(selected.can_complete_app);
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
    crate::settings::set_current_provider(&app, None).unwrap();
    let missing = review
        .review_app_with_state(&inspected, &token, &app, &runtime)
        .unwrap();
    assert!(missing.can_choose_provider);
    assert_eq!(
        missing
            .keep_files_providers
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["managed"]
    );
    let selected = review
        .select_provider(
            &inspected,
            &token,
            &app,
            &missing.revision,
            "managed",
            &runtime,
        )
        .unwrap();
    assert!(selected.can_complete_app);
    let mode = crate::mode::state::mode_state(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "codex",
    )
    .unwrap();
    assert_eq!(mode.proxy_route.as_deref(), Some("b"));
    assert!(!mode.attached);
    for (path, bytes) in native {
        assert_eq!(
            std::fs::read(path).ok(),
            bytes,
            "keep-files choice changed native files"
        );
    }
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_waits_for_owner_without_refreshing() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let first = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    crate::rt::block_on(runtime.codex_oauth_manager.test_refresh_next(
        "synthetic-completion-account",
        "unused-access",
        "unused-refresh",
    ));
    let before = snapshot(f.home.path());
    let next = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    assert!(
        next.can_complete_app,
        "completion proves persisted identity, not remote login validity"
    );
    assert_ne!(
        next.revision, first.revision,
        "manager cache facts bind revision"
    );
    assert!(
        runtime.codex_oauth_manager.test_refresh_is_queued(),
        "review must not refresh"
    );
    assert_eq!(snapshot(f.home.path()), before);
    std::thread::scope(|scope| {
        let (review, inspected, token, runtime) = (&review, &inspected, &token, &runtime);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let mut thread = None;
        crate::rt::block_on(runtime.codex_oauth_manager.with_live_auth_guard(&[], |_| {
            thread = Some(scope.spawn(move || {
                started_tx.send(()).unwrap();
                let result =
                    review.review_app_with_state(inspected, token, &AppType::Codex, runtime);
                done_tx
                    .send(result.map(|view| view.can_complete_app))
                    .unwrap();
            }));
            started_rx.recv().unwrap();
            assert!(matches!(
                done_rx.recv_timeout(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            Ok(())
        }))
        .unwrap();
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
            .unwrap());
        thread.unwrap().join().unwrap();
    });
    assert!(runtime.codex_oauth_manager.test_refresh_is_queued());
    assert_eq!(snapshot(f.home.path()), before);
    let serialized = serde_json::to_string(&next).unwrap();
    for secret in [
        "synthetic-completion-access",
        "test-refresh-token",
        "synthetic-completion-id",
    ] {
        assert!(!serialized.contains(secret));
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_retains_interrupted_choice_recovery() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let app = AppType::Codex;
    crate::mode::state::update_app(
        &f.device,
        &runtime.db.secret_session().read().unwrap(),
        "codex",
        |entry| {
            entry.mode = None;
            Ok(())
        },
    )
    .unwrap();
    let view = review
        .review_app_with_state(&inspected, &token, &app, &runtime)
        .unwrap();
    crate::mode::operation::failpoint::crash_at(Some("upgrade:mode_target"));
    let selected = review.select_mode(
        &inspected,
        &token,
        &app,
        &view.revision,
        &super::super::UpgradeModeChoice {
            mode: crate::mode::state::Mode::Direct,
            proxy_route: None,
        },
        &runtime,
    );
    crate::mode::operation::failpoint::crash_at(None);
    assert!(selected.is_err());
    let interrupted = review
        .review_app_with_state(&inspected, &token, &app, &runtime)
        .unwrap();
    assert_eq!(interrupted.has_pending_operation, Some(true));
    assert!(interrupted.can_recover_operation);
    let recovered =
        review.recover_app_with_state(&inspected, &token, &app, &interrupted.revision, &runtime);
    assert!(
        recovered.is_ok(),
        "original interrupted managed keep-files journal must recover: {:?}",
        recovered.err()
    );
    let fresh = review
        .review_app_with_state(&inspected, &token, &app, &runtime)
        .unwrap();
    assert!(
        fresh.can_complete_app && fresh.has_pending_operation == Some(false),
        "fresh recovery facts: {}",
        serde_json::to_string(&fresh).unwrap()
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_retains_original_native_recovery() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let app = AppType::Codex;
    crate::mode::operation::failpoint::crash_at(Some("target"));
    let switched = crate::services::ProviderService::switch(&runtime, app.clone(), "b");
    crate::mode::operation::failpoint::crash_at(None);
    assert!(switched.is_err());
    let interrupted = review
        .review_app_with_state(&inspected, &token, &app, &runtime)
        .unwrap();
    assert_eq!(interrupted.has_pending_operation, Some(true));
    assert!(interrupted.can_recover_operation);
    let recovered =
        review.recover_app_with_state(&inspected, &token, &app, &interrupted.revision, &runtime);
    assert!(
        recovered.is_ok(),
        "manager-backed query must retain original native recovery revision: {:?}",
        recovered.err()
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&app)
            .unwrap()
            .as_deref(),
        Some("b")
    );
    assert_outgoing_managed_placement_is_still_unproven(&f, &inspected, &review, &token, &runtime);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_generic_admission_is_runtime_safe() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let before = snapshot(f.home.path());
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        drop(crate::mode::operation::AppWrite::begin_mode(&runtime.proxy_service, &AppType::Codex).unwrap());
        runtime.codex_oauth_manager.with_live_auth_guard(&[], |generation| {
            assert!(crate::mode::operation::AppWrite::begin_mode(&runtime.proxy_service, &AppType::Codex).is_err(),
                "an unavailable proof must return rather than nest an executor or reacquire the manager");
            drop(crate::mode::operation::AppWrite::begin_codex_mode(&runtime.proxy_service, generation)?);
            Ok(())
        }).await.unwrap();
    });
    assert_eq!(snapshot(f.home.path()), before);
    let foreign_session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let foreign_db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        foreign_session,
    );
    let foreign_runtime = crate::store::AppState::new(std::sync::Arc::new(foreign_db)).unwrap();
    let before = snapshot(f.home.path());
    assert!(
        review
            .review_app_with_state(&inspected, &token, &AppType::Codex, &foreign_runtime)
            .is_err(),
        "same path and decrypted bytes do not substitute the authenticated runtime session"
    );
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn codex_managed_completion_recovers_cross_account_and_restart() {
    let f = Fixture::new();
    let (inspected, review, token, runtime) = managed_native_runtime(&f);
    let account = "synthetic-second-account";
    crate::rt::block_on(
        runtime
            .codex_oauth_manager
            .add_test_account_with_access_token(
                account,
                "synthetic-second-access",
                Some("synthetic-second-id"),
            ),
    )
    .unwrap();
    let mut row = runtime.db.get_all_providers("codex").unwrap()["managed"].clone();
    row.id = "managed-second".into();
    row.meta
        .as_mut()
        .unwrap()
        .auth_binding
        .as_mut()
        .unwrap()
        .account_id = Some(account.into());
    runtime.db.save_provider("codex", &row).unwrap();
    crate::mode::operation::failpoint::crash_at(Some("target"));
    let switched = crate::services::ProviderService::switch(&runtime, AppType::Codex, &row.id);
    crate::mode::operation::failpoint::crash_at(None);
    assert!(switched.is_err());
    let session = runtime.db.secrets.clone();
    drop(runtime);
    let runtime = crate::store::AppState::new(std::sync::Arc::new(
        Database::init_with_secrets(session).unwrap(),
    ))
    .unwrap();
    let view = review
        .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
        .unwrap();
    assert!(view.can_recover_operation && view.has_pending_operation == Some(true));
    let recovered = review
        .recover_app_with_state(
            &inspected,
            &token,
            &AppType::Codex,
            &view.revision,
            &runtime,
        )
        .unwrap();
    assert!(recovered.can_complete_app && recovered.has_pending_operation == Some(false));
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Codex)
            .unwrap()
            .as_deref(),
        Some(row.id.as_str())
    );
    let before = snapshot(f.home.path());
    assert!(review
        .recover_app_with_state(
            &inspected,
            &token,
            &AppType::Codex,
            &view.revision,
            &runtime
        )
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    // Admission itself neither refreshes nor invents a post-restart access cache.
    drop(
        crate::mode::operation::AppWrite::begin_mode(&runtime.proxy_service, &AppType::Codex)
            .unwrap(),
    );
    assert!(
        review
            .review_app_with_state(&inspected, &token, &AppType::Codex, &runtime)
            .unwrap()
            .can_complete_app
    );
    assert_eq!(snapshot(f.home.path()), before);
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
}
