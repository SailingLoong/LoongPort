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
    fn write_raw_mode(&self, bytes: &[u8]) {
        let session = session::SecretSession::from_context(self.root.clone(), self.vault.clone());
        self.device
            .write_device(
                &session.read().unwrap(),
                &crate::secrets::owned_file::DeviceFile::registered(
                    crate::secrets::owned_file::DEVICE_STATE_FILE,
                )
                .unwrap(),
                bytes,
            )
            .unwrap();
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn schema17_per_app_future_peer_authentication_is_read_only() {
    let f = Fixture::new();
    f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct"},"claude":{"mode":"future-mode","opaque":123456789012345678901234567890}}}"#);
    let inspected = inspect(&f.root, &f.device).unwrap();
    let before = snapshot(f.home.path());
    let review = AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
        .expect("future peer is not a shared authentication failure");
    assert_eq!(review.view(&inspected).unwrap().status, "ready_to_check");
    assert_eq!(snapshot(f.home.path()), before);
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Codex).is_err()
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn schema17_per_app_future_peer_staging_preserves_known_facts_and_unknown_pending() {
    let f = Fixture::new();
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session,
    );
    db.save_provider(
        "codex",
        &crate::provider::Provider::with_id(
            "synthetic-retained".into(),
            "synthetic".into(),
            serde_json::json!({}),
            None,
        ),
    )
    .unwrap();
    db.set_current_provider("codex", "synthetic-retained")
        .unwrap();
    drop(db);
    f.write_settings(&crate::settings::AppSettings {
        current_provider_codex: Some("synthetic-retained".into()),
        ..Default::default()
    });
    let bytes = br#"{"version":1,"apps":{"codex":{"mode":"direct"},"claude":{"mode":"future-mode","pending":{"op":"future-op","opaque":true},"opaque":123456789012345678901234567890}}}"#;
    f.write_raw_mode(bytes);
    let cipher = std::fs::read(f.device.state_path()).unwrap();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let result = review.stage_review(&inspected, &token).unwrap();
    let codex = result
        .apps
        .iter()
        .find(|app| app.app_type == "codex")
        .unwrap();
    assert_eq!(codex.saved_mode, Some(crate::mode::state::Mode::Direct));
    assert_eq!(codex.mode_resolution, "preserved");
    assert_eq!(codex.provider_resolution, "preserved");
    assert!(!codex.requires_mode_choice && !codex.requires_provider_choice);
    assert_eq!(
        serde_json::to_value(codex).unwrap()["hasPendingOperation"],
        false
    );
    let claude = result
        .apps
        .iter()
        .find(|app| app.app_type == "claude")
        .unwrap();
    assert_eq!(claude.saved_mode, None);
    assert_eq!(claude.mode_resolution, "verification_required");
    assert_eq!(claude.provider_resolution, "verification_required");
    assert!(!claude.requires_mode_choice && !claude.requires_provider_choice);
    assert!(serde_json::to_value(claude).unwrap()["hasPendingOperation"].is_null());
    for app in &result.apps {
        assert_eq!(app.default_action, "keep_files");
        assert!(!app.default_takeover);
    }
    let gemini = result
        .apps
        .iter()
        .find(|app| app.app_type == "gemini")
        .unwrap();
    assert_eq!(gemini.mode_resolution, "missing");
    assert_eq!(gemini.saved_mode, None);
    assert_eq!(
        serde_json::to_value(gemini).unwrap()["hasPendingOperation"],
        false
    );
    assert!(!result.can_start_upgrade);
    assert_eq!(snapshot(f.home.path()), before);
    assert_eq!(std::fs::read(f.device.state_path()).unwrap(), cipher);
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Codex).is_err()
    );
    let public = serde_json::to_string(&result).unwrap();
    assert!(!public.contains("synthetic-retained") && !public.contains("future-op"));
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn schema17_per_app_shared_envelope_failure_is_never_repaired() {
    let f = Fixture::new();
    for bytes in [
        br#"{"version":2,"apps":{"codex":{"mode":"direct"}}}"#.as_slice(),
        br#"{"version":1,"apps":[]}"#,
        br#"{"version":1,"apps":{"codex":{},"codex":{}}}"#,
        br#"{"version":1,"apps":{}} trailing"#,
    ] {
        f.write_raw_mode(bytes);
        let inspected = inspect(&f.root, &f.device).unwrap();
        let before = snapshot(f.home.path());
        assert!(
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .is_err()
        );
        assert!(checkpoint::create(&f.root, &f.device, &f.vault, &[]).is_err());
        assert_eq!(snapshot(f.home.path()), before);
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
    upgrade_database_handoff_is_durable_without_freezing_later_app_changes();
    upgrade_checkpoint_preserves_credential_generation_until_handoff_finishes();
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

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_preserves_credential_generation_until_handoff_finishes() {
    let mut outcomes = Vec::new();
    for rotate in [false, true] {
        let f = Fixture::new();
        checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
        let before = snapshot(f.home.path());
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        let result = if rotate {
            crate::secrets::transition::rotate(&db, &f.store, "synthetic-new-password", false)
        } else {
            crate::secrets::rewrap::change_password(&db, &f.store, "synthetic-new-password", false)
        };
        outcomes.push((rotate, matches!(result, Err(AppError::Config(ref code)) if code == "upgrade.checkpoint_pending"), snapshot(f.home.path()) == before));
    }
    assert!(
        outcomes
            .iter()
            .all(|(_, blocked, unchanged)| *blocked && *unchanged),
        "checkpoint must retain its original generation: (rotate, blocked, unchanged)={outcomes:?}"
    );

    let f = Fixture::new();
    let historical = f.root.join("backups/upgrade-checkpoint.json");
    std::fs::create_dir_all(historical.parent().unwrap()).unwrap();
    std::fs::write(&historical, b"unrelated historical backup").unwrap();
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    crate::secrets::rewrap::change_password(&db, &f.store, "synthetic-new-password", false)
        .unwrap();
    assert_ne!(
        db.secret_session().read().unwrap().metadata(),
        f.vault.metadata()
    );
    assert_eq!(
        std::fs::read(historical).unwrap(),
        b"unrelated historical backup"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_database_handoff_is_durable_without_freezing_later_app_changes() {
    use crate::secrets::transition::{self, Checkpoint};
    let f = Fixture::new();
    let id = checkpoint::create(&f.root, &f.device, &f.vault, &[]).unwrap();
    let db_path = f.root.join(crate::config::DB_FILE_NAME);
    let db = Database::from_connection(
        rusqlite::Connection::open(&db_path).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    assert_eq!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault).unwrap(),
        None
    );
    let interrupted =
        checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |at| {
            if at == Checkpoint::Keys {
                Err(AppError::Config("synthetic.interruption".into()))
            } else {
                Ok(())
            }
        });
    assert!(interrupted.is_err());
    assert!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault).is_err(),
        "target CP alone is insufficient while original recovery is pending"
    );
    drop(db);
    transition::recover(&f.root, &f.store, None).unwrap();
    assert_eq!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault).unwrap(),
        Some(id.clone())
    );
    let committed = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
    let db = Database::from_connection(
        rusqlite::Connection::open(&db_path).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    db.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO settings(key,value) VALUES('synthetic-app-change','preserve')",
            [],
        )
        .unwrap();
    {
        let vault = db.secret_session().read().unwrap();
        crate::mode::state::update(&f.device, &vault, |live| {
            live.apps.entry("claude".into()).or_default().pending =
                Some(crate::mode::state::Pending {
                    op: crate::mode::state::op::APPLY.into(),
                    files: vec![],
                    target: Default::default(),
                    published: false,
                    extra: Default::default(),
                });
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault).unwrap(),
        Some(id)
    );
    assert_eq!(
        std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
        committed,
        "query must not rewrite the checkpoint"
    );
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    assert!(checkpoint::ensure_no_pending_checkpoint(&f.device).is_err());
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Claude).is_err()
    );
    db.conn
        .lock()
        .unwrap()
        .pragma_update(None, "user_version", 21)
        .unwrap();
    assert!(checkpoint::verified_database_id(&f.root, &f.device, &f.vault).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_app_resolution_preserves_reliable_mode_and_provider_without_takeover() {
    use crate::mode::state::{AppLiveState, LiveState, Mode};
    let f = Fixture::new();
    let session =
        crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let db = Database::from_connection(source, session.clone());
    for app in ["claude", "codex"] {
        db.save_provider(
            app,
            &crate::provider::Provider::with_id(
                "retained-pointer-canary".into(),
                "synthetic".into(),
                serde_json::json!({}),
                None,
            ),
        )
        .unwrap();
        db.set_current_provider(app, "retained-pointer-canary")
            .unwrap();
    }
    // Modern saved mode remains authoritative over stale legacy proxy flags.
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE proxy_config SET enabled=1 WHERE app_type='claude'",
            [],
        )
        .unwrap();
    drop(db);
    f.write_settings(&crate::settings::AppSettings {
        current_provider_claude: Some("retained-pointer-canary".into()),
        current_provider_codex: Some("retained-pointer-canary".into()),
        ..Default::default()
    });
    let mut live = LiveState::default();
    live.apps.insert(
        "claude".into(),
        AppLiveState {
            mode: Some(Mode::Direct),
            ..Default::default()
        },
    );
    live.apps.insert(
        "codex".into(),
        AppLiveState {
            mode: Some(Mode::Proxy),
            proxy_route: Some("retained-pointer-canary".into()),
            ..Default::default()
        },
    );
    f.device
        .write_device(
            &session.read().unwrap(),
            &crate::secrets::owned_file::DeviceFile::registered(
                crate::secrets::owned_file::DEVICE_STATE_FILE,
            )
            .unwrap(),
            &serde_json::to_vec(&live).unwrap(),
        )
        .unwrap();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let staged = review.stage_review(&inspected, &token).unwrap();
    for (name, mode) in [("claude", Mode::Direct), ("codex", Mode::Proxy)] {
        let app = staged.apps.iter().find(|app| app.app_type == name).unwrap();
        assert_eq!(app.saved_mode, Some(mode));
        assert_eq!(app.mode_resolution, "preserved");
        assert_eq!(app.provider_resolution, "preserved");
        assert!(!app.requires_mode_choice && !app.requires_provider_choice);
        assert_eq!(app.default_action, "keep_files");
        assert!(!app.default_takeover);
    }
    assert!(!staged.can_start_upgrade);
    assert_eq!(snapshot(f.home.path()), before);
    let public = serde_json::to_string(&staged).unwrap();
    assert!(!public.contains("retained-pointer-canary"));
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_app_resolution_isolates_pending_and_future_state_without_direct_defaults() {
    use crate::mode::state::{AppLiveState, LiveState, Mode, Pending};
    let f = Fixture::new();
    let session =
        crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let mut live = LiveState::default();
    live.apps.insert(
        "claude".into(),
        AppLiveState {
            mode: Some(Mode::Direct),
            pending: Some(Pending {
                op: "switch".into(),
                files: vec![],
                target: Default::default(),
                published: true,
                extra: Default::default(),
            }),
            ..Default::default()
        },
    );
    live.apps.insert(
        "codex".into(),
        AppLiveState {
            mode: Some(Mode::Direct),
            extra: serde_json::from_value(serde_json::json!({"future-semantics":true})).unwrap(),
            ..Default::default()
        },
    );
    f.device
        .write_device(
            &session.read().unwrap(),
            &crate::secrets::owned_file::DeviceFile::registered(
                crate::secrets::owned_file::DEVICE_STATE_FILE,
            )
            .unwrap(),
            &serde_json::to_vec(&live).unwrap(),
        )
        .unwrap();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let staged = review.stage_review(&inspected, &token).unwrap();
    for name in ["claude", "codex"] {
        let app = staged.apps.iter().find(|app| app.app_type == name).unwrap();
        assert_eq!(app.mode_resolution, "verification_required");
        assert!(
            !app.requires_mode_choice,
            "a choice must not bypass an unresolved operation or future semantics"
        );
    }
    for name in ["gemini", "grokbuild"] {
        let app = staged.apps.iter().find(|app| app.app_type == name).unwrap();
        assert_eq!(app.mode_resolution, "missing");
        assert_eq!(app.provider_resolution, "missing");
        assert!(app.requires_mode_choice && app.requires_provider_choice);
        assert_eq!(app.saved_mode, None);
        assert_eq!(app.default_action, "keep_files");
        assert!(!app.default_takeover);
    }
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_app_resolution_requests_only_missing_or_conflicting_pointers() {
    use crate::mode::state::{AppLiveState, LiveState, Mode};
    let f = Fixture::new();
    let session =
        crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let db = Database::from_connection(source, session.clone());
    for app in ["claude", "codex", "gemini"] {
        for id in ["local", "database"] {
            db.save_provider(
                app,
                &crate::provider::Provider::with_id(
                    id.into(),
                    "synthetic".into(),
                    serde_json::json!({}),
                    None,
                ),
            )
            .unwrap();
        }
        db.set_current_provider(app, "database").unwrap();
    }
    drop(db);
    f.write_settings(&crate::settings::AppSettings {
        current_provider_claude: Some("local".into()),
        current_provider_codex: Some("removed".into()),
        ..Default::default()
    });
    let mut live = LiveState::default();
    for app in ["claude", "codex", "gemini"] {
        live.apps.insert(
            app.into(),
            AppLiveState {
                mode: Some(Mode::Direct),
                ..Default::default()
            },
        );
    }
    f.device
        .write_device(
            &session.read().unwrap(),
            &crate::secrets::owned_file::DeviceFile::registered(
                crate::secrets::owned_file::DEVICE_STATE_FILE,
            )
            .unwrap(),
            &serde_json::to_vec(&live).unwrap(),
        )
        .unwrap();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    review.prepare_checkpoint(&mut inspected, &token).unwrap();
    let before = snapshot(f.home.path());
    let staged = review.stage_review(&inspected, &token).unwrap();
    for name in ["claude", "codex"] {
        let app = staged.apps.iter().find(|app| app.app_type == name).unwrap();
        assert_eq!(app.mode_resolution, "preserved");
        assert_eq!(app.provider_resolution, "conflict");
        assert!(!app.requires_mode_choice && app.requires_provider_choice);
    }
    let gemini = staged
        .apps
        .iter()
        .find(|app| app.app_type == "gemini")
        .unwrap();
    assert_eq!(
        gemini.provider_resolution, "preserved",
        "unique DB pointer is reliable when local pointer is absent"
    );
    assert!(!gemini.requires_provider_choice);
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn staged_app_resolution_never_infers_mode_from_flags_or_unverified_routes() {
    use crate::mode::state::{AppLiveState, LiveState, Mode};
    for (mode, attached, route, expected_mode, expected_provider) in [
        (None, false, None, "missing", "missing"),
        (Some(Mode::Direct), true, None, "conflict", "missing"),
        (Some(Mode::Proxy), false, None, "preserved", "missing"),
        (
            Some(Mode::Proxy),
            false,
            Some("removed"),
            "preserved",
            "conflict",
        ),
    ] {
        let f = Fixture::new();
        let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        source.execute("UPDATE proxy_config SET enabled=1, auto_failover_enabled=1 WHERE app_type='claude'", []).unwrap();
        drop(source);
        let session =
            crate::secrets::session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let mut live = LiveState::default();
        live.apps.insert(
            "claude".into(),
            AppLiveState {
                mode,
                attached,
                proxy_route: route.map(str::to_owned),
                ..Default::default()
            },
        );
        f.device
            .write_device(
                &session.read().unwrap(),
                &crate::secrets::owned_file::DeviceFile::registered(
                    crate::secrets::owned_file::DEVICE_STATE_FILE,
                )
                .unwrap(),
                &serde_json::to_vec(&live).unwrap(),
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
        let app = staged
            .apps
            .iter()
            .find(|app| app.app_type == "claude")
            .unwrap();
        assert_eq!(app.mode_resolution, expected_mode);
        assert_eq!(app.provider_resolution, expected_provider);
        assert_eq!(
            app.requires_mode_choice,
            matches!(expected_mode, "missing" | "conflict")
        );
        assert_eq!(
            app.requires_provider_choice,
            matches!(expected_provider, "missing" | "conflict")
        );
        assert_eq!(app.default_action, "keep_files");
        assert!(!app.default_takeover);
        assert_eq!(snapshot(f.home.path()), before);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_cancel_is_explicit_bound_and_keeps_runtime_blocked() {
    let f = Fixture::new();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let id = coordinator
        .prepare_upgrade_checkpoint(&token)
        .unwrap()
        .checkpoint_id
        .unwrap();
    let before = snapshot(f.home.path());
    assert!(coordinator.cancel_upgrade_checkpoint("stale", &id).is_err());
    assert!(coordinator
        .cancel_upgrade_checkpoint(&token, "stale")
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let cancelled = coordinator.cancel_upgrade_checkpoint(&token, &id).unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(!cancelled.checkpoint_present);
    assert!(!cancelled.can_check_and_backup && !cancelled.can_start_upgrade);
    assert!(coordinator.verify_runtime_admission_blocked());
    assert_eq!(coordinator.upgrade_view().unwrap().status, "cancelled");
    let after = snapshot(f.home.path());
    let mut expected = before;
    expected.remove(
        f.device
            .root()
            .join(checkpoint::FILE)
            .strip_prefix(f.home.path())
            .unwrap(),
    );
    assert_eq!(after, expected);
    assert_eq!(
        coordinator
            .cancel_upgrade_checkpoint(&token, &id)
            .unwrap()
            .status,
        "cancelled"
    );
    assert_eq!(snapshot(f.home.path()), after);
    assert!(coordinator.prepare_upgrade_checkpoint(&token).is_err());
    assert!(coordinator.review_upgrade_ownership(&token).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_cancel_lost_cleanup_response_requires_explicit_retry() {
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let id = review
        .prepare_checkpoint(&mut inspected, &token)
        .unwrap()
        .checkpoint_id
        .unwrap();
    let before = snapshot(f.home.path());
    assert!(review
        .cancel_checkpoint_with_hook(&mut inspected, &token, &id, &mut |at| {
            if at == checkpoint::CancellationBoundary::Removed {
                Err(AppError::Config("test.lost_response".into()))
            } else {
                Ok(())
            }
        })
        .is_err());
    assert!(!f.device.root().join(checkpoint::FILE).exists());
    let pending = review.view(&inspected).unwrap();
    assert_eq!(pending.status, "cancellation_requires_verification");
    assert!(!pending.can_check_and_backup && !pending.can_start_upgrade);
    let after = snapshot(f.home.path());
    assert_eq!(review.view(&inspected).unwrap().status, pending.status);
    assert_eq!(snapshot(f.home.path()), after);
    assert_eq!(
        review
            .cancel_checkpoint(&mut inspected, &token, &id)
            .unwrap()
            .status,
        "cancelled"
    );
    let mut expected = before;
    expected.remove(
        f.device
            .root()
            .join(checkpoint::FILE)
            .strip_prefix(f.home.path())
            .unwrap(),
    );
    assert_eq!(snapshot(f.home.path()), expected);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_cancel_preserves_changed_sources_and_replacement_artifacts() {
    for replace_checkpoint in [false, true] {
        let f = Fixture::new();
        let mut inspected = inspect(&f.root, &f.device).unwrap();
        let mut review =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = review.view(&inspected).unwrap().review_token.unwrap();
        let id = review
            .prepare_checkpoint(&mut inspected, &token)
            .unwrap()
            .checkpoint_id
            .unwrap();
        if replace_checkpoint {
            std::fs::write(
                f.device.root().join(checkpoint::FILE),
                b"foreign checkpoint",
            )
            .unwrap();
        } else {
            f.write_settings(&crate::settings::AppSettings::default());
        }
        let before = snapshot(f.home.path());
        assert!(review
            .cancel_checkpoint(&mut inspected, &token, &id)
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_cancel_pending_preserves_identity_and_rechecks_before_remove() {
    for replace in [false, true] {
        let f = Fixture::new();
        let mut inspected = inspect(&f.root, &f.device).unwrap();
        let mut review =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = review.view(&inspected).unwrap().review_token.unwrap();
        let id = review
            .prepare_checkpoint(&mut inspected, &token)
            .unwrap()
            .checkpoint_id
            .unwrap();
        let path = f.device.root().join(checkpoint::FILE);
        let original = std::fs::read(&path).unwrap();
        assert!(review
            .cancel_checkpoint_with_hook(&mut inspected, &token, &id, &mut |at| {
                if at == checkpoint::CancellationBoundary::Verified {
                    if replace {
                        std::fs::write(&path, b"foreign replacement").unwrap();
                    } else {
                        return Err(AppError::Config("test.before_remove".into()));
                    }
                }
                Ok(())
            })
            .is_err());
        if replace {
            assert_eq!(std::fs::read(&path).unwrap(), b"foreign replacement");
            assert!(review.view(&inspected).is_err());
            assert!(review
                .cancel_checkpoint(&mut inspected, &token, &id)
                .is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"foreign replacement");
        } else {
            assert_eq!(std::fs::read(&path).unwrap(), original);
            let before = snapshot(f.home.path());
            let view = review.view(&inspected).unwrap();
            assert_eq!(view.status, "cancellation_requires_verification");
            assert!(view.checkpoint_present);
            assert_eq!(view.checkpoint_id.as_deref(), Some(id.as_str()));
            assert!(review.stage_review(&inspected, &token).is_err());
            assert!(review.prepare_checkpoint(&mut inspected, &token).is_err());
            assert_eq!(snapshot(f.home.path()), before);
            assert_eq!(
                review
                    .cancel_checkpoint(&mut inspected, &token, &id)
                    .unwrap()
                    .status,
                "cancelled"
            );
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_checkpoint_cancel_rejects_published_database_without_cleanup() {
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let id = review
        .prepare_checkpoint(&mut inspected, &token)
        .unwrap()
        .checkpoint_id
        .unwrap();
    let source = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    let db = Database::from_connection(
        source,
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |_| Ok(())).unwrap();
    let before = snapshot(f.home.path());
    assert!(review
        .cancel_checkpoint(&mut inspected, &token, &id)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let bytes = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
    assert!(checkpoint::cancel_with_hook(
        &f.root,
        &f.device,
        &f.vault,
        &[],
        (&id, &bytes),
        &mut |_| Ok(())
    )
    .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    assert_eq!(
        Database::get_user_version(&db.conn.lock().unwrap()).unwrap(),
        20
    );
}

fn publish_resume_fixture(f: &Fixture) -> String {
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut review =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = review.view(&inspected).unwrap().review_token.unwrap();
    let id = review
        .prepare_checkpoint(&mut inspected, &token)
        .unwrap()
        .checkpoint_id
        .unwrap();
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |_| Ok(())).unwrap();
    id
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_database_resume_authenticates_completed_boundary_without_runtime_or_app_admission() {
    let f = Fixture::new();
    let id = publish_resume_fixture(&f);
    // Later app facts remain separate from the completed DB proof.
    let mut live = crate::mode::state::LiveState::default();
    live.apps.entry("codex".into()).or_default().mode = Some(crate::mode::state::Mode::Direct);
    let mut json = serde_json::to_value(&live).unwrap();
    json["apps"]["claude"] = serde_json::json!({"mode":"future_mode"});
    let bytes = serde_json::to_vec(&json).unwrap();
    assert!(crate::mode::state::decode(&bytes).is_err());
    let original_session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    f.device
        .write_device(
            &original_session.read().unwrap(),
            &crate::secrets::owned_file::DeviceFile::registered(
                crate::secrets::owned_file::DEVICE_STATE_FILE,
            )
            .unwrap(),
            &bytes,
        )
        .unwrap();
    f.write_settings(&crate::settings::AppSettings {
        current_provider_codex: Some("later-pointer-canary".into()),
        ..Default::default()
    });
    let before = snapshot(f.home.path());
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let candidate = coordinator.upgrade_view().unwrap();
    assert_eq!(candidate.status, "checkpoint_requires_verification");
    assert!(candidate.can_authenticate && candidate.requires_authentication);
    assert!(candidate.checkpoint_id.is_none());
    assert!(coordinator.verify_runtime_admission_blocked());
    assert!(coordinator
        .authenticate_upgrade(Some("wrong synthetic password"), &f.store)
        .is_err());
    let verified = coordinator.authenticate_upgrade(None, &f.store).unwrap();
    assert_eq!(verified.status, "database_verified");
    assert_eq!(verified.checkpoint_id.as_deref(), Some(id.as_str()));
    assert!(verified.checkpoint_present);
    assert!(!verified.requires_authentication && !verified.can_authenticate);
    assert!(!verified.can_check_and_backup && !verified.can_start_upgrade);
    assert!(coordinator.verify_runtime_admission_blocked());
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Codex).is_err()
    );
    assert_eq!(
        coordinator.upgrade_view().unwrap().status,
        "database_verified"
    );
    assert_eq!(snapshot(f.home.path()), before);
    let public = serde_json::to_string(&verified).unwrap();
    assert!(!public.contains("later-pointer-canary") && !public.contains(f.root.to_str().unwrap()));
    let token = verified.review_token.unwrap();
    assert!(coordinator.prepare_upgrade_checkpoint(&token).is_err());
    assert!(coordinator.cancel_upgrade_checkpoint(&token, &id).is_err());
    assert!(coordinator.review_upgrade_ownership(&token).is_err());
    assert_eq!(snapshot(f.home.path()), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_database_resume_rejects_missing_unpublished_corrupt_and_pending_proofs() {
    for scenario in ["missing", "unpublished", "corrupt", "pending"] {
        let f = Fixture::new();
        if scenario == "unpublished" {
            let mut inspected = inspect(&f.root, &f.device).unwrap();
            let mut review =
                AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                    .unwrap();
            let token = review.view(&inspected).unwrap().review_token.unwrap();
            review.prepare_checkpoint(&mut inspected, &token).unwrap();
        } else if scenario != "missing" {
            publish_resume_fixture(&f);
        }
        if scenario == "missing" || scenario == "unpublished" {
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME))
                .unwrap()
                .pragma_update(None, "user_version", 20)
                .unwrap();
        }
        if scenario == "corrupt" {
            std::fs::write(f.device.root().join(checkpoint::FILE), b"foreign proof").unwrap();
        }
        if scenario == "pending" {
            std::fs::write(
                f.root.join(crate::secrets::transition::INTENT),
                b"pending original owner",
            )
            .unwrap();
        }
        let before = snapshot(f.home.path());
        let coordinator = crate::secrets::startup::StartupCoordinator::new(
            f.root.clone(),
            inspect(&f.root, &f.device).unwrap(),
        );
        assert!(
            coordinator.authenticate_upgrade(None, &f.store).is_err(),
            "{scenario}"
        );
        assert_eq!(snapshot(f.home.path()), before, "{scenario}");
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_database_resume_preserves_future_precedence_and_rechecks_checkpoint_drift() {
    for future in [true, false] {
        let f = Fixture::new();
        let id = publish_resume_fixture(&f);
        if future {
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME))
                .unwrap()
                .pragma_update(None, "user_version", 21)
                .unwrap();
            std::fs::write(f.root.join("vault.json"), b"unreadable future vault").unwrap();
            let before = snapshot(f.home.path());
            let coordinator = crate::secrets::startup::StartupCoordinator::new(
                f.root.clone(),
                inspect(&f.root, &f.device).unwrap(),
            );
            assert_eq!(
                coordinator.upgrade_view().unwrap().status,
                "newer_binary_required"
            );
            assert!(coordinator.authenticate_upgrade(None, &f.store).is_err());
            assert_eq!(snapshot(f.home.path()), before);
        } else {
            let coordinator = crate::secrets::startup::StartupCoordinator::new(
                f.root.clone(),
                inspect(&f.root, &f.device).unwrap(),
            );
            let view = coordinator.authenticate_upgrade(None, &f.store).unwrap();
            assert_eq!(view.checkpoint_id.as_deref(), Some(id.as_str()));
            f.write_settings(&crate::settings::AppSettings::default());
            let before = snapshot(f.home.path());
            assert_eq!(
                coordinator.upgrade_view().unwrap().status,
                "database_verified"
            );
            assert_eq!(snapshot(f.home.path()), before);
            std::fs::write(
                f.device.root().join(checkpoint::FILE),
                b"foreign checkpoint",
            )
            .unwrap();
            let changed = snapshot(f.home.path());
            assert!(coordinator.upgrade_view().is_err());
            assert_eq!(snapshot(f.home.path()), changed);
            assert!(coordinator.verify_runtime_admission_blocked());
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn upgrade_database_resume_queries_revalidate_db_vault_and_exact_checkpoint_binding() {
    for scenario in ["database_version", "vault_bytes", "same_id_checkpoint"] {
        let f = Fixture::new();
        let id = publish_resume_fixture(&f);
        let coordinator = crate::secrets::startup::StartupCoordinator::new(
            f.root.clone(),
            inspect(&f.root, &f.device).unwrap(),
        );
        let view = coordinator.authenticate_upgrade(None, &f.store).unwrap();
        assert_eq!(view.checkpoint_id.as_deref(), Some(id.as_str()));
        let db = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        db.execute(
            "INSERT INTO settings(key,value) VALUES('synthetic-later-app','preserved')",
            [],
        )
        .unwrap();
        let before = snapshot(f.home.path());
        assert_eq!(
            coordinator.upgrade_view().unwrap().status,
            "database_verified"
        );
        assert_eq!(snapshot(f.home.path()), before);
        match scenario {
            "database_version" => db.pragma_update(None, "user_version", 21).unwrap(),
            "vault_bytes" => {
                let path = f.root.join("vault.json");
                let mut bytes = std::fs::read(&path).unwrap();
                bytes.push(b'\n');
                std::fs::write(path, bytes).unwrap();
            }
            "same_id_checkpoint" => {
                let file =
                    crate::secrets::owned_file::DeviceFile::registered(checkpoint::FILE).unwrap();
                let path = f.device.root().join(checkpoint::FILE);
                let bytes = std::fs::read(&path).unwrap();
                let plain = file.decode(&f.vault, &bytes).unwrap();
                let replacement = file.encode(&f.vault, &plain).unwrap();
                assert_ne!(replacement, bytes);
                std::fs::write(path, replacement).unwrap();
                assert_eq!(
                    checkpoint::verified_database_id(&f.root, &f.device, &f.vault).unwrap(),
                    Some(id)
                );
            }
            _ => unreachable!(),
        }
        let changed = snapshot(f.home.path());
        assert!(coordinator.upgrade_view().is_err(), "{scenario}");
        assert_eq!(snapshot(f.home.path()), changed, "{scenario}");
        assert!(coordinator.verify_runtime_admission_blocked());
    }
}

fn published_pointer_fixture(f: &Fixture) {
    publish_resume_fixture(f);
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    for id in ["synthetic-before", "synthetic-target"] {
        db.save_provider(
            "codex",
            &crate::provider::Provider::with_id(
                id.into(),
                "synthetic".into(),
                serde_json::json!({}),
                None,
            ),
        )
        .unwrap();
    }
    db.set_current_provider("codex", "synthetic-before")
        .unwrap();
    f.write_settings(&crate::settings::AppSettings {
        current_provider_codex: Some("synthetic-target".into()),
        ..Default::default()
    });
    f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","pending":{"op":"switch","files":[],"target":{"pointer":"synthetic-target"},"published":true}},"claude":{"mode":"future-mode","opaque":123456789012345678901234567890}}}"#);
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_recovery_uses_original_journal_without_runtime_or_file_writes() {
    let f = Fixture::new();
    published_pointer_fixture(&f);
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let settings = std::fs::read(crate::settings::settings_path()).unwrap();
    let before = snapshot(f.home.path());
    let view = coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
        .unwrap();
    assert!(view.can_recover_operation);
    assert_eq!(view.has_pending_operation, Some(true));
    assert!(!view.can_start_upgrade && !view.can_complete_app);
    assert_eq!(snapshot(f.home.path()), before);
    assert!(coordinator
        .recover_upgrade_app(&token, &crate::app_config::AppType::Codex, "stale-revision")
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let recovered = coordinator
        .recover_upgrade_app(&token, &crate::app_config::AppType::Codex, &view.revision)
        .unwrap();
    assert_eq!(recovered.has_pending_operation, Some(false));
    assert_eq!(recovered.saved_mode, Some(crate::mode::state::Mode::Direct));
    assert_eq!(recovered.pointer_consistent, Some(true));
    assert!(!recovered.can_complete_app && !recovered.can_start_upgrade);
    assert_eq!(
        std::fs::read(crate::settings::settings_path()).unwrap(),
        settings
    );
    assert!(coordinator.verify_runtime_admission_blocked());
    assert!(
        crate::settings::get_current_provider_ready(&crate::app_config::AppType::Codex).is_err()
    );
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let vault = session.read().unwrap();
    let raw = f
        .device
        .read_device(
            &vault,
            &crate::secrets::owned_file::DeviceFile::registered(
                crate::secrets::owned_file::DEVICE_STATE_FILE,
            )
            .unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(std::str::from_utf8(&raw)
        .unwrap()
        .contains("123456789012345678901234567890"));
    assert!(crate::mode::state::pending(&f.device, &vault, "codex")
        .unwrap()
        .is_none());
    let public = serde_json::to_string(&recovered).unwrap();
    assert!(!public.contains("synthetic-target") && !public.contains(f.root.to_str().unwrap()));
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_failure_retains_journal_for_explicit_retry() {
    let f = Fixture::new();
    published_pointer_fixture(&f);
    let path = f.root.join(crate::config::DB_FILE_NAME);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER synthetic_recovery_abort BEFORE UPDATE OF is_current ON providers BEGIN SELECT RAISE(ABORT, 'synthetic interruption'); END;").unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let view = coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
        .unwrap();
    assert!(view.can_recover_operation);
    assert!(coordinator
        .recover_upgrade_app(&token, &crate::app_config::AppType::Codex, &view.revision)
        .is_err());
    let retry = coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
        .unwrap();
    assert_eq!(retry.has_pending_operation, Some(true));
    assert!(retry.can_recover_operation);
    assert!(coordinator.verify_runtime_admission_blocked());
    conn.execute_batch("DROP TRIGGER synthetic_recovery_abort;")
        .unwrap();
    let fresh = coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
        .unwrap();
    let done = coordinator
        .recover_upgrade_app(&token, &crate::app_config::AppType::Codex, &fresh.revision)
        .unwrap();
    assert_eq!(done.pointer_consistent, Some(true));
    assert_eq!(done.has_pending_operation, Some(false));
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_rejects_conflicts_and_own_drift_but_isolates_future_peers() {
    let f = Fixture::new();
    published_pointer_fixture(&f);
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","pending":{"op":"switch","files":[],"target":{"pointer":"synthetic-target"},"published":true}},"claude":{"mode":"another-future-mode","opaque":987654321098765432109876543210}}}"#);
    assert_eq!(
        coordinator
            .review_upgrade_app(&token, &app)
            .unwrap()
            .revision,
        view.revision
    );
    let peer = coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Claude)
        .unwrap();
    assert_eq!(peer.has_pending_operation, None);
    assert!(!peer.can_recover_operation && !peer.can_complete_app);
    f.write_settings(&crate::settings::AppSettings {
        current_provider_codex: Some("synthetic-before".into()),
        ..Default::default()
    });
    let before = snapshot(f.home.path());
    assert!(coordinator
        .recover_upgrade_app(&token, &app, &view.revision)
        .is_err());
    let conflict = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert!(!conflict.can_recover_operation);
    assert!(coordinator
        .recover_upgrade_app("wrong-token", &app, &conflict.revision)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    std::fs::write(
        f.device.root().join(checkpoint::FILE),
        b"changed checkpoint",
    )
    .unwrap();
    assert!(coordinator.review_upgrade_app(&token, &app).is_err());
    assert!(coordinator.verify_runtime_admission_blocked());
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_missing_proxy_route_is_unresolved_without_direct_fallback() {
    let f = Fixture::new();
    published_pointer_fixture(&f);
    f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"proxy","pending":{"op":"switch","files":[],"target":{"pointer":"synthetic-target"},"published":true}}}}"#);
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let before = snapshot(f.home.path());
    let view = coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
        .unwrap();
    assert_eq!(view.saved_mode, Some(crate::mode::state::Mode::Proxy));
    assert!(!view.can_recover_operation);
    assert!(coordinator
        .recover_upgrade_app(&token, &crate::app_config::AppType::Codex, &view.revision)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_file_drift_refuses_mutation_and_retains_protected_settings() {
    let f = Fixture::new();
    published_pointer_fixture(&f);
    f.write_settings(&crate::settings::AppSettings {
        current_provider_codex: Some("synthetic-target".into()),
        webdav_sync: Some(crate::settings::WebDavSyncSettings {
            password: "synthetic-settings-canary".into(),
            ..Default::default()
        }),
        ..Default::default()
    });
    let settings_path = crate::settings::settings_path();
    let bytes = std::fs::read(&settings_path).unwrap();
    let mut raw: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(&bytes).unwrap();
    raw.insert(
        "futurePeerSetting".into(),
        serde_json::value::RawValue::from_string("123456789012345678901234567890".into()).unwrap(),
    );
    std::fs::write(&settings_path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    let config = crate::codex_config::get_codex_config_path();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "model = 'synthetic-external-model'\n").unwrap();
    let before = snapshot(f.home.path());
    assert!(coordinator
        .recover_upgrade_app(&token, &app, &view.revision)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let fresh = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert_ne!(fresh.revision, view.revision);
    let settings = std::fs::read(&settings_path).unwrap();
    coordinator
        .recover_upgrade_app(&token, &app, &fresh.revision)
        .unwrap();
    assert_eq!(std::fs::read(&settings_path).unwrap(), settings);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "model = 'synthetic-external-model'\n"
    );
    assert!(!std::str::from_utf8(&settings)
        .unwrap()
        .contains("synthetic-settings-canary"));
    assert!(coordinator.verify_runtime_admission_blocked());
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_no_journal_reports_actual_client_fields_without_completion() {
    let f = Fixture::new();
    publish_resume_fixture(&f);
    let app = crate::app_config::AppType::Claude;
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    let config = serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-client-token","ANTHROPIC_BASE_URL":"https://provider.example.invalid"}});
    db.save_provider(
        "claude",
        &crate::provider::Provider::with_id(
            "synthetic-live".into(),
            "synthetic".into(),
            config.clone(),
            None,
        ),
    )
    .unwrap();
    db.set_current_provider("claude", "synthetic-live").unwrap();
    f.write_settings(&crate::settings::AppSettings {
        current_provider_claude: Some("synthetic-live".into()),
        ..Default::default()
    });
    f.write_raw_mode(
        br#"{"version":1,"apps":{"claude":{"mode":"direct"},"codex":{"mode":"future-mode"}}}"#,
    );
    let client = crate::config::get_claude_settings_path();
    std::fs::create_dir_all(client.parent().unwrap()).unwrap();
    std::fs::write(&client, serde_json::to_vec(&config).unwrap()).unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let before = snapshot(f.home.path());
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert_eq!(view.has_pending_operation, Some(false));
    assert_eq!(view.pointer_consistent, Some(true));
    assert_eq!(view.stored_fields_match, Some(true));
    assert!(!view.can_recover_operation && !view.can_complete_app && !view.can_start_upgrade);
    assert_eq!(snapshot(f.home.path()), before);
    let altered = serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-externally-changed-token","ANTHROPIC_BASE_URL":"https://provider.example.invalid"}});
    std::fs::write(&client, serde_json::to_vec(&altered).unwrap()).unwrap();
    let before = snapshot(f.home.path());
    let changed = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert_eq!(changed.stored_fields_match, Some(false));
    assert_ne!(changed.revision, view.revision);
    assert!(!changed.can_complete_app && !changed.can_start_upgrade);
    assert_eq!(snapshot(f.home.path()), before);
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_refuses_unpublished_future_target_and_credentials_drift() {
    for scenario in [
        "unpublished",
        "future-target",
        "own-journal",
        "vault",
        "transition",
    ] {
        let f = Fixture::new();
        published_pointer_fixture(&f);
        let coordinator = crate::secrets::startup::StartupCoordinator::new(
            f.root.clone(),
            inspect(&f.root, &f.device).unwrap(),
        );
        let token = coordinator
            .authenticate_upgrade(None, &f.store)
            .unwrap()
            .review_token
            .unwrap();
        let app = crate::app_config::AppType::Codex;
        let view = coordinator.review_upgrade_app(&token, &app).unwrap();
        match scenario {
            "unpublished" => f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","pending":{"op":"switch","files":[],"target":{"pointer":"synthetic-target"}}}}}"#),
            "future-target" => f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","pending":{"op":"switch","files":[],"target":{"pointer":"synthetic-target","future":true},"published":true}}}}"#),
            "own-journal" => f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"direct","pending":{"op":"apply","files":[],"target":{"pointer":"synthetic-target"},"published":true}}}}"#),
            "vault" => { let path = f.root.join("vault.json"); let mut bytes = std::fs::read(&path).unwrap(); bytes.push(b'\n'); std::fs::write(path, bytes).unwrap(); },
            "transition" => std::fs::write(f.root.join(crate::secrets::transition::INTENT), b"synthetic pending original owner").unwrap(),
            _ => unreachable!(),
        }
        let before = snapshot(f.home.path());
        assert!(
            coordinator
                .recover_upgrade_app(&token, &app, &view.revision)
                .is_err(),
            "{scenario}"
        );
        if matches!(scenario, "unpublished" | "future-target") {
            let unknown = coordinator.review_upgrade_app(&token, &app).unwrap();
            assert!(!unknown.can_recover_operation, "{scenario}");
        }
        assert_eq!(snapshot(f.home.path()), before, "{scenario}");
        assert!(coordinator.verify_runtime_admission_blocked());
    }
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_future_peer_settings_do_not_block_supported_app() {
    let f = Fixture::new();
    published_pointer_fixture(&f);
    let path = crate::settings::settings_path();
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    raw["currentProviderClaude"] = serde_json::json!({"future":"opaque-peer-pointer"});
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let before = snapshot(f.home.path());
    let view = coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
        .unwrap();
    assert!(view.can_recover_operation);
    assert_eq!(snapshot(f.home.path()), before);
    let bytes = std::fs::read(&path).unwrap();
    coordinator
        .recover_upgrade_app(&token, &crate::app_config::AppType::Codex, &view.revision)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(coordinator
        .review_upgrade_app(&token, &crate::app_config::AppType::Claude)
        .is_err());
    assert!(coordinator.verify_runtime_admission_blocked());
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_future_owned_state_is_refused_before_any_db_write() {
    for payload in [r#""written":{"future":true}"#, r#""stack":{"future":true}"#] {
        let f = Fixture::new();
        published_pointer_fixture(&f);
        f.write_raw_mode(format!(r#"{{"version":1,"apps":{{"codex":{{"mode":"direct",{payload},"pending":{{"op":"switch","files":[],"target":{{"pointer":"synthetic-target"}},"published":true}}}}}}}}"#).as_bytes());
        let coordinator = crate::secrets::startup::StartupCoordinator::new(
            f.root.clone(),
            inspect(&f.root, &f.device).unwrap(),
        );
        let token = coordinator
            .authenticate_upgrade(None, &f.store)
            .unwrap()
            .review_token
            .unwrap();
        let before = snapshot(f.home.path());
        let view = coordinator
            .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
            .unwrap();
        assert!(!view.can_recover_operation);
        assert!(coordinator
            .recover_upgrade_app(&token, &crate::app_config::AppType::Codex, &view.revision)
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
    }
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_rechecks_full_evidence_before_clearing_original_journal() {
    let mut missed = Vec::new();
    for scenario in [
        "attachment",
        "journal",
        "preference",
        "database",
        "database-before-open",
        "flags",
        "cleanup-journal",
        "cleanup-preference",
        "cleanup-vault",
    ] {
        let f = Fixture::new();
        published_pointer_fixture(&f);
        let path = f.root.join(crate::config::DB_FILE_NAME);
        let replacement = f.root.join("synthetic-replacement.db");
        if scenario.starts_with("database") {
            let source = rusqlite::Connection::open(&path).unwrap();
            let mut copy = rusqlite::Connection::open(&replacement).unwrap();
            rusqlite::backup::Backup::new(&source, &mut copy)
                .unwrap()
                .run_to_completion(10, std::time::Duration::ZERO, None)
                .unwrap();
            let db = Database::from_connection(
                copy,
                session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
            );
            db.set_current_provider("codex", "synthetic-target")
                .unwrap();
            crate::proxy::auto_strategy::set_model_pref(
                &db,
                "codex",
                Some("synthetic-stale-preference"),
            )
            .unwrap();
        }
        let coordinator = crate::secrets::startup::StartupCoordinator::new(
            f.root.clone(),
            inspect(&f.root, &f.device).unwrap(),
        );
        let token = coordinator
            .authenticate_upgrade(None, &f.store)
            .unwrap()
            .review_token
            .unwrap();
        let app = crate::app_config::AppType::Codex;
        let view = coordinator.review_upgrade_app(&token, &app).unwrap();
        let root = f.root.clone();
        let vault = f.vault.clone();
        let actual_path = path.clone();
        let injected = std::rc::Rc::new(std::cell::Cell::new(false));
        let injected_in_hook = injected.clone();
        crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
            let expected_point = if scenario == "database-before-open"
                || (cfg!(windows) && scenario == "database")
            {
                // SQLite holds a Windows handle without delete sharing while open.
                // Keep the late replacement on Unix; inject real Windows drift before open.
                "recover:database_open"
            } else if scenario.starts_with("cleanup-") {
                "recover:verified"
            } else {
                "recover:target_committed"
            };
            if point != expected_point {
                return;
            }
            assert!(!injected_in_hook.replace(true), "injected more than once");
            let effect = scenario.strip_prefix("cleanup-").unwrap_or(scenario);
            match effect {
                "attachment" | "journal" => {
                    let op = if effect == "journal" {
                        "apply"
                    } else {
                        "switch"
                    };
                    let attached = effect == "attachment";
                    let bytes = format!(
                        r#"{{"version":1,"apps":{{"codex":{{"mode":"direct","attached":{attached},"pending":{{"op":"{op}","files":[],"target":{{"pointer":"synthetic-target"}},"published":true}}}}}}}}"#
                    );
                    let session = session::SecretSession::from_context(root.clone(), vault.clone());
                    DeviceStore::for_device()
                        .write_device(
                            &session.read().unwrap(),
                            &crate::secrets::owned_file::DeviceFile::registered(
                                crate::secrets::owned_file::DEVICE_STATE_FILE,
                            )
                            .unwrap(),
                            bytes.as_bytes(),
                        )
                        .unwrap();
                }
                "database" | "database-before-open" => {
                    std::fs::rename(&replacement, &actual_path).unwrap();
                    assert!(!replacement.exists());
                }
                "vault" => {
                    let path = root.join("vault.json");
                    let mut bytes = std::fs::read(&path).unwrap();
                    bytes.push(b'\n');
                    std::fs::write(path, bytes).unwrap();
                }
                "preference" | "flags" => {
                    let db = Database::from_connection(
                        rusqlite::Connection::open(&actual_path).unwrap(),
                        session::SecretSession::from_context(root.clone(), vault.clone()),
                    );
                    if effect == "preference" {
                        crate::proxy::auto_strategy::set_model_pref(
                            &db,
                            "codex",
                            Some("synthetic-later-preference"),
                        )
                        .unwrap();
                    } else {
                        db.set_proxy_flags_sync("codex", true, true).unwrap();
                    }
                }
                _ => unreachable!(),
            }
        })));
        let result = coordinator.recover_upgrade_app(&token, &app, &view.revision);
        crate::mode::operation::failpoint::on_boundary(None);
        if !injected.get() {
            missed.push(format!("{scenario}: drift was not injected"));
        }
        if scenario.starts_with("database") {
            let db = Database::from_connection(
                rusqlite::Connection::open(&path).unwrap(),
                session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
            );
            assert_eq!(
                crate::proxy::auto_strategy::get_model_pref_checked(&db, "codex").unwrap(),
                Some("synthetic-stale-preference".to_string()),
                "{scenario}: replacement preference was cleared"
            );
        }
        if result.is_ok() {
            missed.push(format!("{scenario}: accepted drift"));
        }
        let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let pending =
            crate::mode::state::pending(&f.device, &session.read().unwrap(), "codex").unwrap();
        if pending.as_ref().map(|pending| pending.op.as_str())
            != Some(if scenario.ends_with("journal") {
                "apply"
            } else {
                "switch"
            })
        {
            missed.push(format!("{scenario}: original/replacement journal removed"));
        }
        assert!(coordinator.verify_runtime_admission_blocked());
    }
    assert!(missed.is_empty(), "{}", missed.join("; "));
}

#[test]
#[serial_test::serial]
fn u03_published_pointer_retains_reliable_detached_proxy_route() {
    let f = Fixture::new();
    published_pointer_fixture(&f);
    f.write_raw_mode(br#"{"version":1,"apps":{"codex":{"mode":"proxy","proxy_route":"synthetic-before","pending":{"op":"switch","files":[],"target":{"pointer":"synthetic-target"},"published":true}}}}"#);
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert!(view.can_recover_operation);
    let done = coordinator
        .recover_upgrade_app(&token, &app, &view.revision)
        .unwrap();
    assert_eq!(done.saved_mode, Some(crate::mode::state::Mode::Proxy));
    assert_eq!(done.has_pending_operation, Some(false));
    assert_eq!(done.stored_fields_match, None);
    assert!(!done.can_complete_app && !done.can_start_upgrade);
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let mode =
        crate::mode::state::mode_state(&f.device, &session.read().unwrap(), "codex").unwrap();
    assert_eq!(mode.proxy_route.as_deref(), Some("synthetic-before"));
    assert!(!mode.attached);
    assert!(coordinator.verify_runtime_admission_blocked());
}

fn published_client_files_fixture(f: &Fixture) -> Vec<crate::mode::state::PendingFile> {
    published_pointer_fixture(f);
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let vault = session.read().unwrap();
    let mut pending = crate::mode::state::pending(&f.device, &vault, "codex")
        .unwrap()
        .unwrap();
    for (path, bytes) in [
        (
            crate::codex_config::get_codex_auth_path(),
            b"{}\n".as_slice(),
        ),
        (
            crate::codex_config::get_codex_config_path(),
            b"model = \"synthetic-model\"\n".as_slice(),
        ),
    ] {
        crate::config_file_io::write_durable(&path, bytes).unwrap();
        let staged = crate::config_file_io::stage_write(&path, bytes, Some(0o600), true)
            .unwrap()
            .tmp_path()
            .to_owned();
        pending.files.push(crate::mode::state::PendingFile {
            private: Some(true),
            path,
            pre: crate::live::engine::digest(Some(b"synthetic-preimage")),
            planned: crate::live::engine::digest(Some(bytes)),
            staged: Some(staged),
            extra: Default::default(),
        });
    }
    let files = pending.files.clone();
    crate::mode::state::set_pending(&f.device, &vault, "codex", Some(pending)).unwrap();
    files
}

#[test]
#[serial_test::serial]
fn u03_client_files_already_published_complete_original_cleanup_without_rewriting() {
    let f = Fixture::new();
    let files = published_client_files_fixture(&f);
    let witnesses: Vec<_> = files
        .iter()
        .map(|file| inspection::file_revision(&file.path).unwrap())
        .collect();
    let settings = std::fs::read(crate::settings::settings_path()).unwrap();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert!(
        view.can_recover_operation,
        "the original published files prove their planned bytes"
    );
    let result = coordinator
        .recover_upgrade_app(&token, &app, &view.revision)
        .unwrap();
    assert_eq!(result.has_pending_operation, Some(false));
    assert_eq!(result.pointer_consistent, Some(true));
    assert!(!result.can_complete_app && !result.can_start_upgrade);
    for (file, witness) in files.iter().zip(&witnesses) {
        inspection::verify_unchanged(&file.path, witness).unwrap();
        assert!(!file.staged.as_ref().unwrap().exists());
    }
    assert_eq!(
        std::fs::read(crate::settings::settings_path()).unwrap(),
        settings
    );
    assert!(coordinator.verify_runtime_admission_blocked());
    // A lost result is resolved by querying the original owner, not starting again.
    let observed = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert_eq!(observed.has_pending_operation, Some(false));
    assert!(!observed.can_recover_operation && !observed.can_complete_app);
}

#[test]
#[serial_test::serial]
fn u03_client_files_unproven_content_or_staging_keeps_original_intent() {
    for scenario in [
        "preimage",
        "external",
        "staging",
        "foreign-path",
        "unknown-policy",
    ] {
        let f = Fixture::new();
        let files = published_client_files_fixture(&f);
        match scenario {
            "preimage" => std::fs::write(&files[0].path, b"synthetic-preimage").unwrap(),
            "external" => std::fs::write(&files[0].path, b"external generation").unwrap(),
            "staging" => {
                std::fs::write(files[0].staged.as_ref().unwrap(), b"external stage").unwrap()
            }
            "foreign-path" | "unknown-policy" => {
                let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
                let vault = session.read().unwrap();
                let mut pending = crate::mode::state::pending(&f.device, &vault, "codex")
                    .unwrap()
                    .unwrap();
                if scenario == "foreign-path" {
                    pending.files[0].path = f.home.path().join("unowned-client-file");
                } else {
                    pending.files[0].private = None;
                }
                f.write_raw_mode(
                    &serde_json::to_vec(&serde_json::json!({
                        "version": 1,
                        "apps": {"codex": {"mode": "direct", "pending": pending}}
                    }))
                    .unwrap(),
                );
            }
            _ => unreachable!(),
        }
        let coordinator = crate::secrets::startup::StartupCoordinator::new(
            f.root.clone(),
            inspect(&f.root, &f.device).unwrap(),
        );
        let token = coordinator
            .authenticate_upgrade(None, &f.store)
            .unwrap()
            .review_token
            .unwrap();
        let before = snapshot(f.home.path());
        let app = crate::app_config::AppType::Codex;
        let view = coordinator.review_upgrade_app(&token, &app).unwrap();
        assert!(!view.can_recover_operation, "{scenario}");
        assert!(coordinator
            .recover_upgrade_app(&token, &app, &view.revision)
            .is_err());
        assert_eq!(snapshot(f.home.path()), before, "{scenario}");
        assert!(coordinator.verify_runtime_admission_blocked());
    }
}

#[test]
#[serial_test::serial]
fn u03_client_files_late_preimage_is_never_replayed() {
    let f = Fixture::new();
    let files = published_client_files_fixture(&f);
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert!(view.can_recover_operation);
    let path = files[0].path.clone();
    crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
        if point == "recover:begin" {
            std::fs::write(&path, b"synthetic-preimage").unwrap();
        }
    })));
    let result = coordinator.recover_upgrade_app(&token, &app, &view.revision);
    crate::mode::operation::failpoint::on_boundary(None);
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(&files[0].path).unwrap(),
        b"synthetic-preimage"
    );
    for file in &files {
        assert!(file.staged.as_ref().unwrap().exists());
    }
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    assert!(
        crate::mode::state::pending(&f.device, &session.read().unwrap(), "codex")
            .unwrap()
            .is_some()
    );
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session,
    );
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some("synthetic-before")
    );
    assert!(coordinator.verify_runtime_admission_blocked());
}

#[test]
#[serial_test::serial]
fn u03_client_files_replaced_journal_keeps_its_staging_before_cleanup() {
    let f = Fixture::new();
    let files = published_client_files_fixture(&f);
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert!(view.can_recover_operation);
    let root = f.root.clone();
    let vault = f.vault.clone();
    crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
        if point == "recover:verified" {
            let session = session::SecretSession::from_context(root.clone(), vault.clone());
            let vault = session.read().unwrap();
            let store = DeviceStore::for_device();
            let mut pending = crate::mode::state::pending(&store, &vault, "codex")
                .unwrap()
                .unwrap();
            pending.op = "apply".into();
            crate::mode::state::set_pending(&store, &vault, "codex", Some(pending)).unwrap();
        }
    })));
    let result = coordinator.recover_upgrade_app(&token, &app, &view.revision);
    crate::mode::operation::failpoint::on_boundary(None);
    assert!(result.is_err());
    for file in &files {
        assert!(
            file.staged.as_ref().unwrap().exists(),
            "replacement retains original staging"
        );
    }
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    assert_eq!(
        crate::mode::state::pending(&f.device, &session.read().unwrap(), "codex")
            .unwrap()
            .unwrap()
            .op,
        "apply"
    );
    assert!(coordinator.verify_runtime_admission_blocked());
}

#[test]
#[serial_test::serial]
fn u03_client_files_partial_cleanup_failure_retries_without_client_writes() {
    let f = Fixture::new();
    let files = published_client_files_fixture(&f);
    let witnesses: Vec<_> = files
        .iter()
        .map(|file| inspection::file_revision(&file.path).unwrap())
        .collect();
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert!(view.can_recover_operation);
    let mut discards = 0;
    crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
        if point == "discard" {
            discards += 1;
            if discards == 2 {
                crate::mode::operation::failpoint::crash_at(Some("discard"));
            }
        }
    })));
    let result = coordinator.recover_upgrade_app(&token, &app, &view.revision);
    crate::mode::operation::failpoint::on_boundary(None);
    crate::mode::operation::failpoint::crash_at(None);
    assert!(result.is_err());
    assert!(!files[0].staged.as_ref().unwrap().exists());
    assert!(files[1].staged.as_ref().unwrap().exists());
    let observed = coordinator.review_upgrade_app(&token, &app).unwrap();
    assert_eq!(observed.has_pending_operation, Some(true));
    assert!(observed.can_recover_operation);
    let result = coordinator
        .recover_upgrade_app(&token, &app, &observed.revision)
        .unwrap();
    assert_eq!(result.has_pending_operation, Some(false));
    for (file, witness) in files.iter().zip(&witnesses) {
        inspection::verify_unchanged(&file.path, witness).unwrap();
        assert!(!file.staged.as_ref().unwrap().exists());
    }
    assert!(!result.can_complete_app && !result.can_start_upgrade);
    assert!(coordinator.verify_runtime_admission_blocked());
}

#[test]
#[serial_test::serial]
fn u03_client_files_replacement_staging_is_not_deleted_or_acknowledged() {
    for same_bytes in [false, true] {
        let f = Fixture::new();
        let files = published_client_files_fixture(&f);
        let coordinator = crate::secrets::startup::StartupCoordinator::new(
            f.root.clone(),
            inspect(&f.root, &f.device).unwrap(),
        );
        let token = coordinator
            .authenticate_upgrade(None, &f.store)
            .unwrap()
            .review_token
            .unwrap();
        let app = crate::app_config::AppType::Codex;
        let view = coordinator.review_upgrade_app(&token, &app).unwrap();
        let staged = files[0].staged.as_ref().unwrap().clone();
        let replacement = staged.with_extension("synthetic-replacement");
        let bytes = if same_bytes {
            b"{}\n".as_slice()
        } else {
            b"external stage".as_slice()
        };
        std::fs::write(&replacement, bytes).unwrap();
        let replaced = staged.clone();
        let mut applied = false;
        crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
            if point == "discard" && !applied {
                applied = true;
                std::fs::rename(&replacement, &replaced).unwrap();
            }
        })));
        let result = coordinator.recover_upgrade_app(&token, &app, &view.revision);
        crate::mode::operation::failpoint::on_boundary(None);
        assert!(
            result.is_err(),
            "replacement must remain unresolved: same_bytes={same_bytes}"
        );
        assert_eq!(std::fs::read(&staged).unwrap(), bytes);
        assert!(files[1].staged.as_ref().unwrap().exists());
        let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        assert!(
            crate::mode::state::pending(&f.device, &session.read().unwrap(), "codex")
                .unwrap()
                .is_some()
        );
        assert!(coordinator.verify_runtime_admission_blocked());
    }
}

#[test]
#[serial_test::serial]
fn u03_client_files_replaced_before_original_load_is_not_discarded() {
    let f = Fixture::new();
    let files = published_client_files_fixture(&f);
    let coordinator = crate::secrets::startup::StartupCoordinator::new(
        f.root.clone(),
        inspect(&f.root, &f.device).unwrap(),
    );
    let token = coordinator
        .authenticate_upgrade(None, &f.store)
        .unwrap()
        .review_token
        .unwrap();
    let app = crate::app_config::AppType::Codex;
    let view = coordinator.review_upgrade_app(&token, &app).unwrap();
    let root = f.root.clone();
    let vault = f.vault.clone();
    crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
        if point == "recover:load" {
            let session = session::SecretSession::from_context(root.clone(), vault.clone());
            let vault = session.read().unwrap();
            let store = DeviceStore::for_device();
            let mut pending = crate::mode::state::pending(&store, &vault, "codex")
                .unwrap()
                .unwrap();
            pending.op = "apply".into();
            pending.published = false;
            for file in &mut pending.files {
                file.pre = file.planned.clone();
            }
            crate::mode::state::set_pending(&store, &vault, "codex", Some(pending)).unwrap();
        }
    })));
    let result = coordinator.recover_upgrade_app(&token, &app, &view.revision);
    crate::mode::operation::failpoint::on_boundary(None);
    assert!(result.is_err());
    for file in &files {
        assert!(file.staged.as_ref().unwrap().exists());
    }
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let pending = crate::mode::state::pending(&f.device, &session.read().unwrap(), "codex")
        .unwrap()
        .unwrap();
    assert_eq!(pending.op, "apply");
    assert!(!pending.published);
    assert!(coordinator.verify_runtime_admission_blocked());
}
