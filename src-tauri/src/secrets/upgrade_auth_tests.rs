use super::review_tests::snapshot;
use super::*;
use crate::secrets::testing::{MemoryKeyStore, TestHome};

struct Fixture {
    root: std::path::PathBuf,
    device: DeviceStore,
    store: MemoryKeyStore,
    vault: VaultContext,
    _app_root: Option<crate::app_store::TestAppConfigRoot>,
    home: TestHome,
}
impl Fixture {
    fn new() -> Self {
        Self::with_migration(true)
    }
    fn with_migration(complete: bool) -> Self {
        Self::with_custom_root(complete, false)
    }
    fn with_custom_root(complete: bool, custom: bool) -> Self {
        let home = TestHome::new().unwrap();
        let app_root = custom
            .then(|| crate::app_store::TestAppConfigRoot::enter(home.path().join("shared-data")));
        let root = crate::config::get_app_config_dir();
        assert!(root.starts_with(home.path()));
        let store = MemoryKeyStore::default();
        let session = crate::secrets::session::SecretSession::open(&root, &store, None).unwrap();
        let vault = session.read().unwrap().clone();
        let conn = rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
        Database::create_tables_on_conn(&conn).unwrap();
        Database::apply_schema_migrations_on_conn(&conn).unwrap();
        database::loongport_schema::apply(&conn).unwrap();
        database::vault::stamp(&conn, &vault).unwrap();
        drop(conn);
        if complete {
            session.complete_migration().unwrap();
        }
        Self {
            root,
            device: DeviceStore::for_device(),
            store,
            vault,
            _app_root: app_root,
            home,
        }
    }
    fn write_settings(&self, settings: &crate::settings::AppSettings) {
        let bytes = crate::settings::encode_settings_with_vault(settings, &self.vault).unwrap();
        crate::config_file_io::write_durable(&crate::settings::settings_path(), &bytes).unwrap();
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_authentication_is_read_only_and_does_not_publish_runtime_settings() {
    let f = Fixture::new();
    let settings = crate::settings::AppSettings {
        current_provider_claude: Some("synthetic-current".into()),
        ..Default::default()
    };
    f.write_settings(&settings);
    let inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f.home.path());
    let review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let view = review.view(&inspected).unwrap();
    assert_eq!(view.status, "ready_to_check");
    assert!(view.can_check_and_backup && !view.can_start_upgrade && !view.can_authenticate);
    assert!(!view.requires_authentication);
    assert!(view
        .review_token
        .as_ref()
        .is_some_and(|token| !token.is_empty()));
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Claude).is_err(),
        "authentication must not unlock the runtime settings owner"
    );
    assert_eq!(snapshot(f.home.path()), before);
    assert!(!f.device.root().join(checkpoint::FILE).exists());
    let public = serde_json::to_string(&view).unwrap();
    for secret in [
        "synthetic-current",
        f.root.to_str().unwrap(),
        "vault",
        "metadata",
    ] {
        assert!(!public.contains(secret));
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_authentication_rejects_credentials_and_settings_corruption_without_repair() {
    let f = Fixture::new();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f.home.path());
    assert!(AuthenticatedUpgrade::authenticate(
        &f.root,
        &f.device,
        &inspected,
        &f.store,
        Some("wrong synthetic password")
    )
    .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    std::fs::write(
        crate::settings::settings_path(),
        br#"{"webdavSync":"invalid-protected-value"}"#,
    )
    .unwrap();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f.home.path());
    assert!(
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).is_err()
    );
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_authenticated_review_binds_settings_and_client_selection() {
    let f = Fixture::new();
    let mut settings = crate::settings::AppSettings::default();
    let claude = f.home.path().join("custom-claude");
    let codex = f.home.path().join("custom-codex");
    settings.claude_config_dir = Some(claude.to_string_lossy().into_owned());
    settings.codex_config_dir = Some(codex.to_string_lossy().into_owned());
    f.write_settings(&settings);
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("claude.json"), b"{}").unwrap();
    std::fs::create_dir_all(codex.join("legacy")).unwrap();
    std::fs::write(
        codex.join("config.toml"),
        b"model_catalog_json = 'legacy/cc-switch-model-catalog.json'\n",
    )
    .unwrap();
    std::fs::write(codex.join("legacy/cc-switch-model-catalog.json"), b"{}").unwrap();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let paths: Vec<_> = review
        .inputs
        .iter()
        .map(|input| input.path.clone())
        .collect();
    assert!(paths.contains(&crate::settings::settings_path()));
    assert!(paths.contains(&claude.join("claude.json")));
    assert!(paths.contains(&codex.join("legacy/cc-switch-model-catalog.json")));
    assert!(paths.contains(&crate::codex_config::get_codex_managed_oauth_live_auth_marker_path()));
    std::fs::write(claude.join("settings.json"), b"{}").unwrap();
    assert!(
        review.view(&inspected).is_err(),
        "appearance of preferred filename must invalidate the reviewed inventory"
    );
    assert!(!f.device.root().join(checkpoint::FILE).exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_authenticated_review_does_not_follow_external_catalogs() {
    let f = Fixture::new();
    let codex = f.home.path().join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    let external = f.home.path().join("unowned/cc-switch-model-catalog.json");
    std::fs::create_dir_all(&external).unwrap();
    let config = format!(
        "model_catalog_json = {}\n",
        serde_json::to_string(&external.to_string_lossy()).unwrap()
    );
    std::fs::write(codex.join("config.toml"), config).unwrap();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f.home.path());
    let review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    assert!(!review.inputs.iter().any(|input| input.path == external));
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_authentication_refuses_pending_credential_migration_without_completing_it() {
    let f = Fixture::with_migration(false);
    let inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f.home.path());
    let result = AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None);
    assert!(matches!(result, Err(AppError::Config(code)) if code == "secret.migration_required"));
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_action_is_explicit_token_bound_and_idempotent() {
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let before = snapshot(f.home.path());
    assert!(review
        .prepare_checkpoint(&mut inspected, "stale review token")
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let completed = review.prepare_checkpoint(&mut inspected, &token).unwrap();
    assert_eq!(completed.status, "checkpoint_ready");
    assert!(completed.checkpoint_id.is_some());
    assert!(!completed.can_check_and_backup && !completed.can_start_upgrade);
    let after = snapshot(f.home.path());
    let repeated = review.prepare_checkpoint(&mut inspected, &token).unwrap();
    assert_eq!(completed.checkpoint_id, repeated.checkpoint_id);
    assert_eq!(
        review.view(&inspected).unwrap().checkpoint_id,
        completed.checkpoint_id
    );
    assert_eq!(snapshot(f.home.path()), after);
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Claude).is_err()
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_action_refuses_settings_changed_after_authentication() {
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    f.write_settings(&crate::settings::AppSettings::default());
    let before = snapshot(f.home.path());
    assert!(review.prepare_checkpoint(&mut inspected, &token).is_err());
    assert_eq!(snapshot(f.home.path()), before);
    assert!(!f.device.root().join(checkpoint::FILE).exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn startup_coordinator_owns_review_and_never_publishes_runtime() {
    let f = Fixture::new();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(f.root.clone(), inspected);
    assert!(coordinator.upgrade_view().unwrap().can_authenticate);
    let before = snapshot(f.home.path());
    assert!(coordinator
        .authenticate_upgrade(Some("wrong synthetic password"), &f.store)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    assert!(coordinator.upgrade_view().unwrap().can_authenticate);
    let view = coordinator.authenticate_upgrade(None, &f.store).unwrap();
    assert!(view.can_check_and_backup);
    assert_eq!(snapshot(f.home.path()), before);
    assert!(coordinator.verify_runtime_admission_blocked());
    let token = view.review_token.unwrap();
    assert!(coordinator.prepare_upgrade_checkpoint("stale").is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let completed = coordinator.prepare_upgrade_checkpoint(&token).unwrap();
    assert_eq!(completed.status, "checkpoint_ready");
    assert!(coordinator.verify_runtime_admission_blocked());
    assert_eq!(
        coordinator.upgrade_view().unwrap().checkpoint_id,
        completed.checkpoint_id
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_lost_response_retains_artifact_and_reconciles_without_rewrite() {
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let result = review.prepare_checkpoint_with_hook(&mut inspected, &token, &mut |boundary| {
        if boundary == checkpoint::Boundary::Published {
            return Err(AppError::Config("test.lost_response".into()));
        }
        Ok(())
    });
    assert!(matches!(result, Err(AppError::Config(code)) if code == "test.lost_response"));
    let after = snapshot(f.home.path());
    let ready = review.view(&inspected).unwrap();
    assert_eq!(ready.status, "checkpoint_ready");
    assert_eq!(
        ready.checkpoint_id,
        review
            .prepare_checkpoint(&mut inspected, &token)
            .unwrap()
            .checkpoint_id
    );
    assert_eq!(snapshot(f.home.path()), after);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_publication_does_not_acknowledge_other_source_changes() {
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let result = review.prepare_checkpoint_with_hook(&mut inspected, &token, &mut |boundary| {
        if boundary == checkpoint::Boundary::Published {
            f.write_settings(&crate::settings::AppSettings::default());
        }
        Ok(())
    });
    assert!(result.is_err());
    assert!(f.device.root().join(checkpoint::FILE).exists());
    assert!(review.view(&inspected).is_err());
    let after = snapshot(f.home.path());
    assert!(review.prepare_checkpoint(&mut inspected, &token).is_err());
    assert_eq!(snapshot(f.home.path()), after);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn checkpoint_directory_acknowledgement_preserves_absence_and_ancestor_identity() {
    let f = Fixture::new();
    let directory = f.home.path().join("absent-device");
    let settings = directory.join("settings.json");
    let original = inspection::file_revision(&settings).unwrap();
    crate::config_file_io::ensure_private_directory(&directory).unwrap();
    assert!(inspection::verify_unchanged(&settings, &original).is_err());
    let acknowledged =
        inspection::acknowledge_checkpoint_directory(&settings, &original, &directory).unwrap();
    inspection::verify_unchanged(&settings, &acknowledged).unwrap();
    std::fs::write(directory.join("settings.json-wal"), b"unexpected sidecar").unwrap();
    assert!(
        inspection::acknowledge_checkpoint_directory(&settings, &original, &directory).is_err()
    );
    assert!(inspection::verify_unchanged(&settings, &acknowledged).is_err());
    let another = f.home.path().join("another");
    std::fs::create_dir(&another).unwrap();
    assert!(inspection::acknowledge_checkpoint_directory(&settings, &original, &another).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_custom_data_root_preserves_absent_local_settings() {
    let f = Fixture::with_custom_root(true, true);
    assert!(!f.device.root().exists());
    assert_ne!(f.root, f.device.root());
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f.home.path());
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    assert_eq!(snapshot(f.home.path()), before);
    assert!(review.prepare_checkpoint(&mut inspected, "stale").is_err());
    assert!(!f.device.root().exists());
    let ready = review.prepare_checkpoint(&mut inspected, &token).unwrap();
    assert_eq!(ready.status, "checkpoint_ready");
    assert!(!crate::settings::settings_path().exists());
    assert_eq!(
        review.view(&inspected).unwrap().checkpoint_id,
        ready.checkpoint_id
    );
    let after = snapshot(f.home.path());
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    assert_eq!(snapshot(f.home.path()), after);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn startup_upgrade_rejects_existing_incomplete_or_corrupt_checkpoint_without_replacement() {
    let f = Fixture::new();
    checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(f.root.clone(), inspected);
    let before = snapshot(f.home.path());
    assert!(coordinator.authenticate_upgrade(None, &f.store).is_err());
    assert_eq!(snapshot(f.home.path()), before);
    assert!(coordinator.verify_runtime_admission_blocked());
    std::fs::write(
        f.device.root().join(checkpoint::FILE),
        b"corrupt checkpoint",
    )
    .unwrap();
    let inspected = inspect(&f.root, &f.device).unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(f.root.clone(), inspected);
    let before = snapshot(f.home.path());
    assert!(coordinator.authenticate_upgrade(None, &f.store).is_err());
    assert_eq!(snapshot(f.home.path()), before);
    assert!(coordinator.verify_runtime_admission_blocked());
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify() -> Result<(), AppError> {
    upgrade_publication_refuses_memory_target();
    #[cfg(unix)]
    upgrade_publication_keeps_intent_after_atomic_database_replacement();
    upgrade_recovery_rejects_missing_or_inconsistent_stage();
    upgrade_checkpoint_replacement_is_bound_to_the_authenticated_ciphertext();
    upgrade_publication_keeps_intent_when_late_inputs_change();
    upgrade_database_publication_recovers_through_original_generation_boundaries();
    println!("PASS bounded DB/checkpoint publication, original generation recovery, late conflicts, stage integrity and checkpoint CAS");
    codex_declared_field_review_does_not_call_quote_style_a_conflict();
    dotenv_review_literal_matrix_does_not_claim_unknown_values();
    #[cfg(unix)]
    staged_symlink_catalog_and_unrelated_provider_never_prove_ownership();
    staged_gemini_and_grok_projection_facts_do_not_write_clients();
    staged_provider_field_review_uses_pure_projection_and_preserves_user_fields();
    staged_catalog_review_distinguishes_owned_candidate_and_unclaimed_pointers();
    staged_live_review_isolates_invalid_app_and_never_reports_missing_as_direct();
    authenticated_stage_preserves_duplicate_database_current_candidates();
    authenticated_stage_preserves_original_missing_flags_and_unknown_mode();
    authenticated_stage_keeps_stale_local_pointer_as_a_fact_without_runtime_fallback();
    upgrade_authentication_is_read_only_and_does_not_publish_runtime_settings();
    upgrade_authentication_rejects_credentials_and_settings_corruption_without_repair();
    upgrade_authenticated_review_binds_settings_and_client_selection();
    upgrade_authenticated_review_does_not_follow_external_catalogs();
    upgrade_authentication_refuses_pending_credential_migration_without_completing_it();
    upgrade_checkpoint_action_is_explicit_token_bound_and_idempotent();
    upgrade_checkpoint_action_refuses_settings_changed_after_authentication();
    startup_coordinator_owns_review_and_never_publishes_runtime();
    upgrade_checkpoint_lost_response_retains_artifact_and_reconciles_without_rewrite();
    upgrade_checkpoint_publication_does_not_acknowledge_other_source_changes();
    checkpoint_directory_acknowledgement_preserves_absence_and_ancestor_identity();
    upgrade_checkpoint_custom_data_root_preserves_absent_local_settings();
    startup_upgrade_rejects_existing_incomplete_or_corrupt_checkpoint_without_replacement();
    crate::secrets::startup::verify_original_startup_admission();
    println!("PASS startup upgrade authentication and reviewed inputs remain read-only");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn authenticated_stage_preserves_original_missing_flags_and_unknown_mode() {
    let f = Fixture::new();
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    source
        .execute("DELETE FROM proxy_config WHERE app_type='claude'", [])
        .unwrap();
    source
        .execute(
            "UPDATE proxy_config SET enabled=1, auto_failover_enabled=1 WHERE app_type='codex'",
            [],
        )
        .unwrap();
    drop(source);
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    assert!(
        review.stage_review(&inspected, &token).is_err(),
        "verified checkpoint is mandatory"
    );
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    assert!(review.stage_review(&inspected, "stale token").is_err());
    let staged = review.stage_review(&inspected, &token).unwrap();
    assert_eq!(staged.source_versions.upstream, 17);
    assert_eq!(staged.staged_versions.upstream, 20);
    assert_eq!(staged.staged_versions.loongport, 24);
    assert!(!staged.can_start_upgrade);
    let claude = staged
        .apps
        .iter()
        .find(|app| app.app_type == "claude")
        .unwrap();
    assert_eq!(claude.legacy_proxy_enabled, None);
    assert_eq!(claude.saved_mode, None);
    let codex = staged
        .apps
        .iter()
        .find(|app| app.app_type == "codex")
        .unwrap();
    assert_eq!(codex.legacy_proxy_enabled, Some(true));
    assert_eq!(codex.legacy_failover_enabled, Some(true));
    assert_eq!(codex.saved_mode, None);
    assert!(staged.apps.iter().all(|app| app.saved_mode.is_none()));
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Claude).is_err()
    );
    assert_eq!(snapshot(f.home.path()), before);
    review.stage_review(&inspected, &token).unwrap();
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn authenticated_stage_keeps_stale_local_pointer_as_a_fact_without_runtime_fallback() {
    let f = Fixture::new();
    let settings = crate::settings::AppSettings {
        current_provider_claude: Some("secret-pointer-canary".into()),
        ..Default::default()
    };
    f.write_settings(&settings);
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let staged = review.stage_review(&inspected, &token).unwrap();
    let claude = staged
        .apps
        .iter()
        .find(|app| app.app_type == "claude")
        .unwrap();
    assert!(claude.local_current_present);
    assert_eq!(claude.local_current_exists, Some(false));
    assert_eq!(claude.database_current_count, 0);
    let serialized = serde_json::to_string(&staged).unwrap();
    assert!(!serialized.contains("secret-pointer-canary"));
    assert!(!serialized.contains(f.root.to_str().unwrap()));
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn authenticated_stage_preserves_duplicate_database_current_candidates() {
    let f = Fixture::new();
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let session =
        crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let db = Database::from_connection(source, session);
    for id in ["a", "b"] {
        db.save_provider(
            "claude",
            &crate::provider::Provider::with_id(
                id.into(),
                "synthetic row".into(),
                serde_json::json!({"env":{"ANTHROPIC_API_KEY":"stage-secret-canary"}}),
                None,
            ),
        )
        .unwrap();
    }
    drop(db);
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    source
        .execute(
            "UPDATE providers SET is_current=1 WHERE app_type='claude'",
            [],
        )
        .unwrap();
    drop(source);
    f.write_settings(&crate::settings::AppSettings {
        current_provider_claude: Some("a".into()),
        ..Default::default()
    });
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let staged = review.stage_review(&inspected, &token).unwrap();
    let claude = staged
        .apps
        .iter()
        .find(|app| app.app_type == "claude")
        .unwrap();
    assert_eq!(claude.provider_count, 2);
    assert_eq!(claude.local_current_exists, Some(true));
    assert_eq!(claude.database_current_count, 2);
    assert_eq!(claude.local_matches_database_current, None);
    assert_eq!(claude.saved_mode, None);
    assert!(!serde_json::to_string(&staged)
        .unwrap()
        .contains("stage-secret-canary"));
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_live_review_isolates_invalid_app_and_never_reports_missing_as_direct() {
    let f = Fixture::new();
    let claude = f.home.path().join(".claude");
    let gemini = f.home.path().join(".gemini");
    let codex = f.home.path().join(".codex");
    for path in [&claude, &gemini, &codex] {
        std::fs::create_dir(path).unwrap();
    }
    std::fs::write(claude.join("settings.json"), b"{ secret-parser-canary").unwrap();
    std::fs::write(
        gemini.join(".env"),
        format!(
            "GEMINI_API_KEY=\"{}\" # user comment\nGEMINI_SANDBOX=docker\n",
            crate::live::project::claude::PROXY_TOKEN_PLACEHOLDER
        ),
    )
    .unwrap();
    std::fs::write(codex.join("config.toml"), b"model = 'synthetic'\n").unwrap();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let staged = review.stage_review(&inspected, &token).unwrap();
    let app = |name: &str| staged.apps.iter().find(|app| app.app_type == name).unwrap();
    assert_eq!(app("claude").live_status, "invalid");
    assert_eq!(app("claude").legacy_takeover_marker, None);
    assert_eq!(app("gemini").live_status, "parsed");
    assert_eq!(app("gemini").legacy_takeover_marker, Some(true));
    assert_eq!(app("codex").live_status, "parsed");
    assert_eq!(app("codex").legacy_takeover_marker, Some(false));
    assert_eq!(app("grokbuild").live_status, "missing");
    assert_eq!(app("grokbuild").legacy_takeover_marker, None);
    assert!(staged.apps.iter().all(|app| app.saved_mode.is_none()));
    assert!(!serde_json::to_string(&staged)
        .unwrap()
        .contains("secret-parser-canary"));
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_catalog_review_distinguishes_owned_candidate_and_unclaimed_pointers() {
    for (filename, claimed, expected) in [
        ("loongport-model-catalog.json", false, "managed"),
        ("cc-switch-model-catalog.json", false, "managed"),
        (
            "outside/loongport-model-catalog.json",
            false,
            "unclaimed_external",
        ),
        (
            "outside/loongport-model-catalog.json",
            true,
            "candidate_external",
        ),
    ] {
        let f = Fixture::new();
        let codex = f.home.path().join(".codex");
        std::fs::create_dir(&codex).unwrap();
        let external = filename.starts_with("outside/");
        let target = if external {
            f.home.path().join(filename)
        } else {
            codex.join(filename)
        };
        if external {
            std::fs::create_dir_all(&target).unwrap();
        } else {
            std::fs::write(&target, b"{\"models\":[]}").unwrap();
        }
        let config = format!(
            "model_catalog_json = {}\n",
            serde_json::to_string(&target.to_string_lossy()).unwrap()
        );
        std::fs::write(codex.join("config.toml"), &config).unwrap();
        if claimed {
            let source =
                rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
            let session = crate::secrets::session::SecretSession::from_context(
                f.root.clone(),
                f.vault.clone(),
            );
            let db = Database::from_connection(source, session);
            db.save_provider(
                "codex",
                &crate::provider::Provider::with_id(
                    "candidate".into(),
                    "synthetic".into(),
                    serde_json::json!({"auth":{},"config":config}),
                    None,
                ),
            )
            .unwrap();
            drop(db);
            f.write_settings(&crate::settings::AppSettings {
                current_provider_codex: Some("candidate".into()),
                ..Default::default()
            });
        }
        let mut inspected = inspect(&f.root, &f.device).unwrap();
        let mut review =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = review.view(&inspected).unwrap().review_token.unwrap();
        review.prepare_checkpoint(&mut inspected, &token).unwrap();
        let before = snapshot(f.home.path());
        let staged = review.stage_review(&inspected, &token).unwrap();
        let codex = staged
            .apps
            .iter()
            .find(|app| app.app_type == "codex")
            .unwrap();
        assert_eq!(codex.catalog_ownership, Some(expected));
        assert_eq!(codex.catalog_file_present, (!external).then_some(true));
        assert_eq!(codex.saved_mode, None);
        assert_eq!(
            codex.stored_fields_match, None,
            "no known field conflict is not a full Codex route/auth/catalog proof"
        );
        assert_eq!(snapshot(f.home.path()), before);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_provider_field_review_uses_pure_projection_and_preserves_user_fields() {
    for matches in [true, false] {
        let f = Fixture::new();
        let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        let session =
            crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let db = Database::from_connection(source, session);
        db.save_provider("claude", &crate::provider::Provider::with_id("row".into(), "synthetic".into(), serde_json::json!({"env":{"ANTHROPIC_API_KEY":"expected-canary"},"hooks":{"row":"not-owned"}}), None)).unwrap();
        db.save_provider(
            "codex",
            &crate::provider::Provider::with_id(
                "row".into(),
                "synthetic".into(),
                serde_json::json!({"auth":{},"config":"model = 'saved-model'\n"}),
                None,
            ),
        )
        .unwrap();
        drop(db);
        f.write_settings(&crate::settings::AppSettings {
            current_provider_claude: Some("row".into()),
            current_provider_codex: Some("row".into()),
            ..Default::default()
        });
        let claude = f.home.path().join(".claude");
        let codex = f.home.path().join(".codex");
        std::fs::create_dir(&claude).unwrap();
        std::fs::create_dir(&codex).unwrap();
        std::fs::write(claude.join("settings.json"), serde_json::to_vec(&serde_json::json!({"env":{"ANTHROPIC_API_KEY":if matches {"expected-canary"} else {"external-canary"}},"hooks":{"live":"preserve-user-data"}})).unwrap()).unwrap();
        std::fs::write(
            codex.join("config.toml"),
            "model = 'saved-model'\nbase_url = 'https://legacy.invalid'\n[projects]\nkeep = true\n",
        )
        .unwrap();
        let mut inspected = inspect(&f.root, &f.device).unwrap();
        let mut review =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = review.view(&inspected).unwrap().review_token.unwrap();
        review.prepare_checkpoint(&mut inspected, &token).unwrap();
        let before = snapshot(f.home.path());
        let staged = review.stage_review(&inspected, &token).unwrap();
        let app = |name: &str| staged.apps.iter().find(|app| app.app_type == name).unwrap();
        assert_eq!(app("claude").stored_fields_match, Some(matches));
        assert_eq!(
            app("codex").stored_fields_match,
            Some(false),
            "live-only floor keys must not disappear from the review"
        );
        let serialized = serde_json::to_string(&staged).unwrap();
        assert!(!serialized.contains("expected-canary") && !serialized.contains("external-canary"));
        assert_eq!(snapshot(f.home.path()), before);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_gemini_and_grok_projection_facts_do_not_write_clients() {
    for (key, gemini_expected, matches) in [
        ("gemini-canary", Some(true), true),
        ("external", Some(false), false),
        ("${FROM_ENV}", None, true),
    ] {
        let f = Fixture::new();
        let grok = "[models]\ndefault = 'synthetic'\n[model.synthetic]\nmodel = 'synthetic-model'\nname = 'Synthetic'\nbase_url = 'https://synthetic.invalid/v1'\napi_key = 'grok-canary'\napi_backend = 'responses'\ncontext_window = 200000\n";
        let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        let session =
            crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let db = Database::from_connection(source, session);
        db.save_provider("gemini", &crate::provider::Provider::with_id("row".into(), "synthetic".into(), serde_json::json!({"env":{"GEMINI_API_KEY":"gemini-canary","GOOGLE_GEMINI_BASE_URL":"https://synthetic.invalid"},"config":{"model":{"name":"synthetic-model"}}}), None)).unwrap();
        db.save_provider(
            "grokbuild",
            &crate::provider::Provider::with_id(
                "row".into(),
                "synthetic".into(),
                serde_json::json!({"config":grok}),
                None,
            ),
        )
        .unwrap();
        drop(db);
        f.write_settings(&crate::settings::AppSettings {
            current_provider_gemini: Some("row".into()),
            current_provider_grokbuild: Some("row".into()),
            ..Default::default()
        });
        std::fs::create_dir_all(
            crate::gemini_config::get_gemini_env_path()
                .parent()
                .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(crate::grok_config::get_grok_config_path().parent().unwrap())
            .unwrap();
        std::fs::write(crate::gemini_config::get_gemini_env_path(), format!("GEMINI_API_KEY=\"{}\" # user comment\nGOOGLE_GEMINI_BASE_URL=https://synthetic.invalid\nUSER_OPTION=keep\n", key)).unwrap();
        std::fs::write(crate::gemini_config::get_gemini_settings_path(), br#"{"security":{"auth":{"selectedType":"gemini-api-key"}},"model":{"name":"synthetic-model"},"user":{"keep":true}}"#).unwrap();
        std::fs::write(
            crate::grok_config::get_grok_config_path(),
            format!(
                "{}\n[ui]\nkeep = true\n",
                if matches {
                    grok.to_owned()
                } else {
                    grok.replace("grok-canary", "external")
                }
            ),
        )
        .unwrap();
        let inspected = inspect(&f.root, &f.device).unwrap();
        let coordinator =
            crate::secrets::startup::StartupCoordinator::new(f.root.clone(), inspected);
        let token = coordinator
            .authenticate_upgrade(None, &f.store)
            .unwrap()
            .review_token
            .unwrap();
        coordinator.prepare_upgrade_checkpoint(&token).unwrap();
        let before = snapshot(f.home.path());
        assert!(coordinator.review_upgrade_ownership("stale token").is_err());
        let staged = coordinator.review_upgrade_ownership(&token).unwrap();
        for name in ["gemini", "grokbuild"] {
            assert_eq!(
                staged
                    .apps
                    .iter()
                    .find(|app| app.app_type == name)
                    .unwrap()
                    .stored_fields_match,
                if name == "gemini" {
                    gemini_expected
                } else {
                    Some(matches)
                },
                "{name}"
            );
        }
        if gemini_expected.is_none() {
            assert_eq!(
                staged
                    .apps
                    .iter()
                    .find(|app| app.app_type == "gemini")
                    .unwrap()
                    .legacy_takeover_marker,
                None
            );
        }
        assert!(!staged.can_start_upgrade);
        assert!(coordinator.verify_runtime_admission_blocked());
        assert_eq!(snapshot(f.home.path()), before);
    }
}

#[cfg(unix)]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_symlink_catalog_and_unrelated_provider_never_prove_ownership() {
    let f = Fixture::new();
    let codex = f.home.path().join(".codex");
    let outside = f.home.path().join("outside-catalog");
    std::fs::create_dir(&codex).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(
        outside.join("cc-switch-model-catalog.json"),
        b"external-canary",
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, codex.join("linked")).unwrap();
    let config = "model_catalog_json = 'linked/cc-switch-model-catalog.json'\n";
    std::fs::write(codex.join("config.toml"), config).unwrap();
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let session =
        crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let db = Database::from_connection(source, session);
    db.save_provider(
        "codex",
        &crate::provider::Provider::with_id(
            "unrelated".into(),
            "synthetic".into(),
            serde_json::json!({"auth":{},"config":config}),
            None,
        ),
    )
    .unwrap();
    drop(db);
    f.write_settings(&crate::settings::AppSettings {
        current_provider_codex: Some("missing".into()),
        ..Default::default()
    });
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let staged = review.stage_review(&inspected, &token).unwrap();
    let codex_facts = staged
        .apps
        .iter()
        .find(|app| app.app_type == "codex")
        .unwrap();
    assert_eq!(codex_facts.catalog_ownership, Some("unclaimed_external"));
    assert_eq!(codex_facts.catalog_file_present, None);
    assert_eq!(codex_facts.local_current_exists, Some(false));
    assert_eq!(std::fs::read_link(codex.join("linked")).unwrap(), outside);
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
fn dotenv_review_literal_matrix_does_not_claim_unknown_values() {
    for (source, expected) in [
        (
            "GEMINI_API_KEY=\"PROXY_MANAGED\" # comment\n",
            Some("PROXY_MANAGED"),
        ),
        (
            "GEMINI_API_KEY='PROXY_MANAGED' # comment\n",
            Some("PROXY_MANAGED"),
        ),
        ("GEMINI_API_KEY=`PROXY_MANAGED`\n", Some("PROXY_MANAGED")),
        (
            "export GEMINI_API_KEY=PROXY_MANAGED\n",
            Some("PROXY_MANAGED"),
        ),
        ("export\tGEMINI_API_KEY=PROXY_MANAGED\n", None),
        ("GEMINI_API_KEY: PROXY_MANAGED\n", None),
        ("GEMINI_API_KEY=\"${FROM_ENV}\"\n", None),
        ("GEMINI_API_KEY=\"escaped\\nvalue\"\n", None),
        ("GEMINI_API_KEY=PROXY_MANAGED # comment\n", None),
        ("GEMINI_API_KEY=\n\"PROXY_MANAGED\"\n", None),
        ("GEMINI_API_KEY='first\nsecond'\n", Some("first\nsecond")),
        (
            "USER_DATA=\"first\nGEMINI_API_KEY=PROXY_MANAGED\nlast\"\nGEMINI_API_KEY=real\n",
            Some("real"),
        ),
        (
            "USER.DATA: \"first\nGEMINI_API_KEY=PROXY_MANAGED\nlast\"\nGEMINI_API_KEY=real\n",
            Some("real"),
        ),
        (
            "GEMINI_API_KEY=PROXY_MANAGED\nGEMINI_API_KEY=${FROM_ENV}\n",
            None,
        ),
    ] {
        let entries = crate::live::patch::dotenv::literal_owned_entries(
            source,
            crate::live::floor::gemini_floor_env,
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "GEMINI_API_KEY");
        assert_eq!(entries[0].1.as_deref(), expected);
    }
}

#[cfg_attr(test, test)]
fn codex_declared_field_review_does_not_call_quote_style_a_conflict() {
    let candidate = crate::provider::Provider::with_id(
        "synthetic".into(),
        "synthetic".into(),
        serde_json::json!({"auth":{},"config":"model = \"same-model\"\nmodel_context_window = 100000\n"}),
        None,
    );
    let live = serde_json::json!({"auth":{},"config":"model = 'same-model'\nmodel_context_window = 100_000\n"});
    assert_eq!(
        super::projection_review::compare(
            &crate::app_config::AppType::Codex,
            Some(&candidate),
            &live
        ),
        None
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_database_publication_recovers_through_original_generation_boundaries() {
    use crate::secrets::transition::{self, Checkpoint};
    for boundary in [
        Checkpoint::Staged,
        Checkpoint::Intent,
        Checkpoint::Database,
        Checkpoint::Artifact(1),
        Checkpoint::Artifact(2),
        Checkpoint::Metadata,
        Checkpoint::Keys,
    ] {
        let f = Fixture::with_custom_root(true, boundary == Checkpoint::Database);
        let vault = std::sync::RwLock::new(f.vault.clone());
        crate::mode::state::update(&f.device, &vault.read().unwrap(), |live| {
            live.apps.entry("grok-build".into()).or_default().mode =
                Some(crate::mode::state::Mode::Direct);
            Ok(())
        })
        .unwrap();
        let state_bytes = std::fs::read(f.device.state_path()).unwrap();
        let keys = ExistingKeysOnly(&f.store);
        let client = crate::config::get_claude_settings_path();
        crate::config_file_io::ensure_private_directory(client.parent().unwrap()).unwrap();
        std::fs::write(&client, br#"{"userOwned":"preserve"}"#).unwrap();
        let unrelated = f.root.join("backups/unrelated.db");
        std::fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        std::fs::write(&unrelated, b"opaque unrelated backup").unwrap();
        let id = checkpoint::create(&f.root, &f.device, &f.vault, std::slice::from_ref(&client))
            .unwrap();
        let vault_bytes = std::fs::read(f.root.join("vault.json")).unwrap();
        let source_checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session,
        );
        let result =
            checkpoint::publish_database_with_hook(&db, &f.device, &keys, &id, &mut |at| {
                if at == boundary {
                    Err(AppError::Config("synthetic.interruption".into()))
                } else {
                    Ok(())
                }
            });
        assert!(
            matches!(result, Err(AppError::Config(ref code)) if code == "synthetic.interruption"),
            "must reach original {boundary:?} boundary: {result:?}"
        );
        if boundary == Checkpoint::Staged {
            assert!(!f.root.join(transition::INTENT).exists());
            assert_eq!(
                std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
                source_checkpoint
            );
            checkpoint::publish_database_with_hook(&db, &f.device, &keys, &id, &mut |_| Ok(()))
                .unwrap();
        } else {
            assert!(f.root.join(transition::INTENT).exists());
            drop(db);
            transition::recover(&f.root, &keys, None).unwrap();
        }
        assert_eq!(
            checkpoint::published_database_id(&f.root, &f.device, &f.vault).unwrap(),
            Some(id)
        );
        assert_eq!(
            std::fs::read(f.root.join("vault.json")).unwrap(),
            vault_bytes
        );
        assert_eq!(
            std::fs::read(&client).unwrap(),
            br#"{"userOwned":"preserve"}"#
        );
        assert_eq!(
            std::fs::read(&unrelated).unwrap(),
            b"opaque unrelated backup"
        );
        assert_eq!(std::fs::read(f.device.state_path()).unwrap(), state_bytes);
        assert!(!f.root.join(transition::INTENT).exists());
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
        assert!(
            crate::settings::get_current_provider_ready(&crate::app_config::AppType::Claude)
                .is_err()
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_publication_keeps_intent_when_late_inputs_change() {
    use crate::secrets::transition::{self, Checkpoint};
    for database_changed in [false, true] {
        let f = Fixture::new();
        let client = crate::config::get_claude_settings_path();
        crate::config_file_io::ensure_private_directory(client.parent().unwrap()).unwrap();
        std::fs::write(&client, b"{}").unwrap();
        let id = checkpoint::create(&f.root, &f.device, &f.vault, std::slice::from_ref(&client))
            .unwrap();
        let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let db_path = f.root.join(crate::config::DB_FILE_NAME);
        let db = Database::from_connection(rusqlite::Connection::open(&db_path).unwrap(), session);
        let result =
            checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |at| {
                if database_changed && at == Checkpoint::Keys {
                    let conn = rusqlite::Connection::open(&db_path).unwrap();
                    conn.execute(
                        "INSERT INTO settings(key,value) VALUES('synthetic-later','preserve')",
                        [],
                    )
                    .unwrap();
                } else if !database_changed && at == Checkpoint::Database {
                    std::fs::write(&client, br#"{"later":"preserve"}"#).unwrap();
                }
                Ok(())
            });
        assert!(
            result.is_err(),
            "late changes cannot erase the original publication intent"
        );
        assert!(f.root.join(transition::INTENT).exists());
        drop(db);
        assert!(transition::recover(&f.root, &f.store, None).is_err());
        assert!(f.root.join(transition::INTENT).exists());
        if database_changed {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            let value: String = conn
                .query_row(
                    "SELECT value FROM settings WHERE key='synthetic-later'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(value, "preserve");
        } else {
            assert_eq!(std::fs::read(&client).unwrap(), br#"{"later":"preserve"}"#);
        }
    }
}

struct ExistingKeysOnly<'a>(&'a MemoryKeyStore);
impl crate::secrets::key_store::KeyStore for ExistingKeysOnly<'_> {
    fn load(
        &self,
        vault: &str,
        key: &str,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, crate::secrets::key_store::KeyStoreError> {
        crate::secrets::key_store::KeyStore::load(self.0, vault, key)
    }
    fn save(
        &self,
        _: &str,
        _: &str,
        _: &[u8],
    ) -> Result<(), crate::secrets::key_store::KeyStoreError> {
        panic!("schema publication must not write the key store")
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), crate::secrets::key_store::KeyStoreError> {
        panic!("schema publication must not remove keys")
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_recovery_rejects_missing_or_inconsistent_stage() {
    use crate::secrets::transition::{self, Checkpoint};
    for missing in [true, false] {
        let f = Fixture::new();
        let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
        let db_path = f.root.join(crate::config::DB_FILE_NAME);
        let source_cp = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let db = Database::from_connection(rusqlite::Connection::open(&db_path).unwrap(), session);
        assert!(
            checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |at| {
                if at == Checkpoint::Intent {
                    Err(AppError::Config("synthetic.interruption".into()))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        drop(db);
        let intent_path = f.root.join(transition::INTENT);
        let mut intent: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&intent_path).unwrap()).unwrap();
        let operation = intent["id"].as_str().unwrap().to_owned();
        let stage = f
            .root
            .join(format!(".vault-transition-{operation}/0.stage"));
        if missing {
            std::fs::remove_file(&stage).unwrap();
        } else {
            // Authenticate an internally inconsistent target to exercise the
            // semantic target binding, independently of the stage-byte hash.
            let conn = rusqlite::Connection::open(&stage).unwrap();
            conn.execute(
                "INSERT INTO settings(key,value) VALUES('synthetic-stage','different')",
                [],
            )
            .unwrap();
            drop(conn);
            let plain = f
                .vault
                .open(
                    &["local", "generation-transition", &operation],
                    intent["manifest"].as_str().unwrap(),
                )
                .unwrap();
            let mut manifest: serde_json::Value = serde_json::from_slice(&plain).unwrap();
            manifest["artifacts"][0]["digest"] = serde_json::json!(crate::live::engine::digest(
                Some(&std::fs::read(&stage).unwrap())
            )
            .unwrap());
            intent["manifest"] = serde_json::json!(f
                .vault
                .seal(
                    &["local", "generation-transition", &operation],
                    &serde_json::to_vec(&manifest).unwrap()
                )
                .unwrap());
            std::fs::write(&intent_path, serde_json::to_vec(&intent).unwrap()).unwrap();
        }
        assert!(transition::recover(&f.root, &f.store, None).is_err());
        assert!(intent_path.exists());
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        assert_eq!(Database::get_user_version(&conn).unwrap(), 17);
        assert_eq!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
            source_cp
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_replacement_is_bound_to_the_authenticated_ciphertext() {
    use crate::secrets::{owned_file::DeviceFile, transition};
    let f = Fixture::new();
    let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    let file = DeviceFile::registered(checkpoint::FILE).unwrap();
    let cp_path = f.device.root().join(checkpoint::FILE);
    let source_bytes = std::fs::read(&cp_path).unwrap();
    let source_plain = file.decode(&f.vault, &source_bytes).unwrap();
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session,
    );
    let mut later_bytes = Vec::new();
    let result = transition::install_upgrade_database(
        &db,
        &f.store,
        f.device.root(),
        |_, current, _| checkpoint::stage(&f.root, &f.device, current, &id),
        &mut |target, current| {
            let mut payload: serde_json::Value = serde_json::from_slice(&source_plain).unwrap();
            payload["published_database"] = serde_json::json!(Database::content_digest(target)?);
            let ciphertext = file.encode(current, &serde_json::to_vec(&payload).unwrap())?;
            later_bytes = file.encode(current, &source_plain)?;
            std::fs::write(&cp_path, &later_bytes).unwrap();
            Ok(transition::UpgradeCheckpointReplacement {
                source_digest: crate::live::engine::digest(Some(&source_bytes)).unwrap(),
                ciphertext,
            })
        },
        &mut |_| Ok(()),
    );
    assert!(matches!(result, Err(AppError::Config(ref code)) if code == "upgrade.source_changed"));
    assert_eq!(std::fs::read(&cp_path).unwrap(), later_bytes);
    assert!(!f.root.join(transition::INTENT).exists());
    assert_eq!(
        Database::get_user_version(&db.conn.lock().unwrap()).unwrap(),
        17
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_publication_refuses_memory_target() {
    let f = Fixture::new();
    let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    let source_cp = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let mut memory = rusqlite::Connection::open_in_memory().unwrap();
    database::vault::copy(&source, &mut memory).unwrap();
    let db = Database::from_connection(
        memory,
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    assert!(
        checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |_| Ok(()))
            .is_err()
    );
    assert!(!f.root.join(crate::secrets::transition::INTENT).exists());
    assert_eq!(
        std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
        source_cp
    );
    assert_eq!(Database::get_user_version(&source).unwrap(), 17);
}

#[cfg(unix)]
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_publication_keeps_intent_after_atomic_database_replacement() {
    use crate::secrets::transition::{self, Checkpoint};
    let f = Fixture::new();
    let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    let path = f.root.join(crate::config::DB_FILE_NAME);
    let db = Database::from_connection(
        rusqlite::Connection::open(&path).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    let result = checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |at| {
        if at == Checkpoint::Keys {
            let replacement = f.root.join("synthetic-replacement.db");
            let source = rusqlite::Connection::open(&path).unwrap();
            let mut target = rusqlite::Connection::open(&replacement).unwrap();
            database::vault::copy(&source, &mut target).unwrap();
            target
                .execute(
                    "INSERT INTO settings(key,value) VALUES('synthetic-replaced','preserve')",
                    [],
                )
                .unwrap();
            drop(target);
            drop(source);
            std::fs::rename(replacement, &path).unwrap();
        }
        Ok(())
    });
    assert!(
        result.is_err(),
        "publication must read back the current path, not an obsolete SQLite handle"
    );
    assert!(f.root.join(transition::INTENT).exists());
    drop(db);
    assert!(transition::recover(&f.root, &f.store, None).is_err());
    let current = rusqlite::Connection::open(&path).unwrap();
    let value: String = current
        .query_row(
            "SELECT value FROM settings WHERE key='synthetic-replaced'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(value, "preserve");
}
