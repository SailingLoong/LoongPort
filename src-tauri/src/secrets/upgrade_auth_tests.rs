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
    assert!(!completed.can_check_and_backup && completed.can_start_upgrade);
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
        let result = checkpoint::publish_database_with_hook(
            &db,
            &f.device,
            &keys,
            &id,
            &mut |at| {
                if at == boundary {
                    Err(AppError::Config("synthetic.interruption".into()))
                } else {
                    Ok(())
                }
            },
            None,
        );
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
            checkpoint::publish_database_with_hook(
                &db,
                &f.device,
                &keys,
                &id,
                &mut |_| Ok(()),
                None,
            )
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
        let result = checkpoint::publish_database_with_hook(
            &db,
            &f.device,
            &f.store,
            &id,
            &mut |at| {
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
            },
            None,
        );
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
        assert!(checkpoint::publish_database_with_hook(
            &db,
            &f.device,
            &f.store,
            &id,
            &mut |at| {
                if at == Checkpoint::Intent {
                    Err(AppError::Config("synthetic.interruption".into()))
                } else {
                    Ok(())
                }
            },
            None
        )
        .is_err());
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
        None,
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
    assert!(checkpoint::publish_database_with_hook(
        &db,
        &f.device,
        &f.store,
        &id,
        &mut |_| Ok(()),
        None
    )
    .is_err());
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
    let result = checkpoint::publish_database_with_hook(
        &db,
        &f.device,
        &f.store,
        &id,
        &mut |at| {
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
        },
        None,
    );
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
    let interrupted = checkpoint::publish_database_with_hook(
        &db,
        &f.device,
        &f.store,
        &id,
        &mut |at| {
            if at == Checkpoint::Keys {
                Err(AppError::Config("synthetic.interruption".into()))
            } else {
                Ok(())
            }
        },
        None,
    );
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
    checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |_| Ok(()), None)
        .unwrap();
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
    checkpoint::publish_database_with_hook(&db, &f.device, &f.store, &id, &mut |_| Ok(()), None)
        .unwrap();
    id
}

#[test]
#[serial_test::serial]
fn authenticated_explicit_database_publication_reuses_original_checkpoint_and_session() {
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
        .publish_checkpoint(&mut inspected, "stale-token", &id, &f.store)
        .is_err());
    assert!(review
        .publish_checkpoint(&mut inspected, &token, "stale-checkpoint", &f.store)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let client_bytes = review
        .inputs
        .iter()
        .map(|input| (input.path.clone(), std::fs::read(&input.path).ok()))
        .collect::<Vec<_>>();
    let vault_bytes = std::fs::read(f.root.join("vault.json")).unwrap();
    let view = review
        .publish_checkpoint(&mut inspected, &token, &id, &ExistingKeysOnly(&f.store))
        .unwrap();
    assert_eq!(view.status, "database_verified");
    assert_eq!(view.checkpoint_id.as_deref(), Some(id.as_str()));
    assert_ne!(view.review_token.as_deref(), Some(token.as_str()));
    assert_eq!(
        std::fs::read(f.root.join("vault.json")).unwrap(),
        vault_bytes
    );
    assert!(review.inputs.is_empty());
    for (path, bytes) in client_bytes {
        assert_eq!(std::fs::read(path).ok(), bytes);
    }
    assert_eq!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault)
            .unwrap()
            .as_deref(),
        Some(id.as_str())
    );
    assert!(inspected.ensure_runtime_admitted().is_err());
    let before = snapshot(f.home.path());
    assert!(review
        .publish_checkpoint(&mut inspected, &token, &id, &f.store)
        .is_err());
    assert!(review
        .cancel_checkpoint(&mut inspected, view.review_token.as_ref().unwrap(), &id)
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
}

#[test]
#[serial_test::serial]
fn authenticated_database_publication_interruption_queries_and_recovers_original_intent() {
    for boundary in [
        crate::secrets::transition::Checkpoint::Database,
        crate::secrets::transition::Checkpoint::Keys,
    ] {
        let mut f = Fixture::new();
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        crate::secrets::rewrap::change_password(&db, &f.store, "synthetic-recovery-password", true)
            .unwrap();
        drop(db);
        f.vault =
            session::authenticate_existing(&f.root, &f.store, Some("synthetic-recovery-password"))
                .unwrap();
        f.write_raw_mode(br#"{"version":1,"apps":{"claude":{"mode":"direct"},"codex":{"mode":"future-mode","pending":{"op":"future-op","opaque":true}}}}"#);
        let state_bytes = std::fs::read(f.device.state_path()).unwrap();
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        let before_rotation = snapshot(f.home.path());
        assert!(crate::secrets::transition::rotate(
            &db,
            &f.store,
            "synthetic-new-key-password",
            false,
        )
        .is_err());
        assert_eq!(snapshot(f.home.path()), before_rotation);
        drop(db);
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
        let result = review.publish_checkpoint_with_hook(
            &mut inspected,
            &token,
            &id,
            &ExistingKeysOnly(&f.store),
            &mut |at| {
                if at == boundary {
                    Err(AppError::Config(
                        "synthetic.lost-publication-response".into(),
                    ))
                } else {
                    Ok(())
                }
            },
        );
        assert!(
            matches!(&result, Err(AppError::Config(code)) if code == "synthetic.lost-publication-response"),
            "original boundary {boundary:?} not reached: {:?}",
            result.as_ref().err()
        );
        let before = snapshot(f.home.path());
        let view = review.view(&inspected).unwrap();
        assert_eq!(view.status, "recovery_required");
        assert!(!view.can_start_upgrade && !view.can_check_and_backup);
        assert!(review
            .publish_checkpoint(&mut inspected, &token, &id, &f.store)
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        let UpgradeInspection::RecoveryRequired(evidence) = &inspected else {
            panic!("original intent must remain");
        };
        let recovery_token = evidence.token();
        assert!(evidence
            .recover(
                &f.root,
                "stale-recovery-token",
                &f.store,
                "synthetic-recovery-password"
            )
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        assert!(evidence
            .recover(
                &f.root,
                &recovery_token,
                &f.store,
                "synthetic-wrong-password"
            )
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        let recovered = evidence
            .recover(
                &f.root,
                &recovery_token,
                &ExistingKeysOnly(&f.store),
                "synthetic-recovery-password",
            )
            .unwrap();
        assert!(recovered.is_database_resume_candidate());
        assert!(recovered.ensure_runtime_admitted().is_err());
        assert_eq!(std::fs::read(f.device.state_path()).unwrap(), state_bytes);
        assert_eq!(
            checkpoint::verified_database_id(&f.root, &f.device, &f.vault)
                .unwrap()
                .as_deref(),
            Some(id.as_str())
        );
    }
}

#[test]
#[serial_test::serial]
fn authenticated_database_coordinator_reuses_original_recovery_after_publication_failure() {
    let mut f = Fixture::new();
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
    );
    crate::secrets::rewrap::change_password(&db, &f.store, "synthetic-recovery-password", true)
        .unwrap();
    drop(db);
    f.vault =
        session::authenticate_existing(&f.root, &f.store, Some("synthetic-recovery-password"))
            .unwrap();
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
    let result = coordinator.publish_upgrade_checkpoint_with_hook(
        &token,
        &id,
        &ExistingKeysOnly(&f.store),
        Some(&mut |at| {
            if at == crate::secrets::transition::Checkpoint::Keys {
                Err(AppError::Config(
                    "synthetic.lost-publication-response".into(),
                ))
            } else {
                Ok(())
            }
        }),
    );
    assert!(result.is_err());
    assert_eq!(
        coordinator.upgrade_view().unwrap().status,
        "recovery_required"
    );
    let before = snapshot(f.home.path());
    let recovery = serde_json::to_value(coordinator.recovery_view().unwrap()).unwrap();
    assert_eq!(recovery["status"], "pending");
    assert_eq!(recovery["canRecover"], true);
    assert_eq!(snapshot(f.home.path()), before);
    assert!(coordinator
        .recover_operation(
            "stale-recovery-token",
            "synthetic-recovery-password",
            &f.store
        )
        .is_err());
    assert_eq!(snapshot(f.home.path()), before);
    let result = coordinator
        .recover_operation(
            recovery["token"].as_str().unwrap(),
            "synthetic-recovery-password",
            &ExistingKeysOnly(&f.store),
        )
        .unwrap();
    let completed = serde_json::to_value(result).unwrap();
    assert_eq!(completed["status"], "completed");
    assert_eq!(completed["canRecover"], false);
    assert_eq!(completed["restartRequired"], true);
    assert!(coordinator.verify_runtime_admission_blocked());
    assert_eq!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault)
            .unwrap()
            .as_deref(),
        Some(id.as_str())
    );
}

#[test]
#[serial_test::serial]
fn authenticated_database_coordinator_does_not_capture_recovery_before_its_intent() {
    for foreign in [false, true] {
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
        let checkpoint_bytes = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        assert!(coordinator
            .publish_upgrade_checkpoint_with_hook(
                &token,
                &id,
                &f.store,
                Some(&mut |at| {
                    if at == crate::secrets::transition::Checkpoint::Staged {
                        if foreign {
                            std::fs::write(
                                f.root.join(crate::secrets::transition::INTENT),
                                b"synthetic-foreign-intent",
                            )
                            .unwrap();
                        }
                        return Err(AppError::Config("synthetic.before-original-intent".into()));
                    }
                    Ok(())
                }),
            )
            .is_err());
        assert_eq!(
            Database::get_user_version(
                &rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap()
            )
            .unwrap(),
            17
        );
        assert_eq!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
            checkpoint_bytes
        );
        assert!(
            matches!(coordinator.recovery_view(), Err(ref code) if code == "secret.no_pending_operation")
        );
        let before = snapshot(f.home.path());
        if foreign {
            assert!(coordinator.upgrade_view().is_err());
            let UpgradeInspection::RecoveryRequired(evidence) =
                inspect(&f.root, &f.device).unwrap()
            else {
                panic!("foreign intent must remain");
            };
            assert!(coordinator
                .recover_operation(&evidence.token(), "synthetic-password", &f.store)
                .is_err());
            assert!(coordinator
                .publish_upgrade_checkpoint(&token, &id, &f.store)
                .is_err());
            assert!(coordinator.authenticate_upgrade(None, &f.store).is_err());
        } else {
            assert_eq!(
                coordinator.upgrade_view().unwrap().status,
                "checkpoint_ready"
            );
            assert!(!f.root.join(crate::secrets::transition::INTENT).exists());
        }
        assert_eq!(snapshot(f.home.path()), before);
        assert!(coordinator.verify_runtime_admission_blocked());
    }
}

#[test]
#[serial_test::serial]
fn authenticated_database_coordinator_never_adopts_replaced_or_additional_journals() {
    for replace in [true, false] {
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
        let mut original = Vec::new();
        assert!(coordinator
            .publish_upgrade_checkpoint_with_hook(
                &token,
                &id,
                &f.store,
                Some(&mut |at| {
                    if at != crate::secrets::transition::Checkpoint::Intent {
                        return Ok(());
                    }
                    let path = f.root.join(crate::secrets::transition::INTENT);
                    original = std::fs::read(&path).unwrap();
                    if replace {
                        let mut intent: serde_json::Value =
                            serde_json::from_slice(&original).unwrap();
                        intent["id"] = serde_json::json!("synthetic-different-operation");
                        std::fs::write(&path, serde_json::to_vec(&intent).unwrap()).unwrap();
                    } else {
                        std::fs::write(f.root.join(".vault-rewrap"), b"{}").unwrap();
                    }
                    Err(AppError::Config(
                        "synthetic.lost-publication-response".into(),
                    ))
                }),
            )
            .is_err());
        assert!(!original.is_empty());
        let before = snapshot(f.home.path());
        let recovery = serde_json::to_value(coordinator.recovery_view().unwrap()).unwrap();
        assert_eq!(recovery["status"], "verification_required");
        assert_eq!(recovery["canRecover"], false);
        let UpgradeInspection::RecoveryRequired(current) = inspect(&f.root, &f.device).unwrap()
        else {
            panic!("foreign journal must remain");
        };
        // Neither the current foreign token nor the owner's retained token may
        // replay another operation. Queries and refusals preserve all bytes.
        assert!(coordinator
            .recover_operation(&current.token(), "synthetic-password", &f.store)
            .is_err());
        assert!(coordinator
            .recover_operation(
                recovery["token"].as_str().unwrap(),
                "synthetic-password",
                &f.store
            )
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        assert!(coordinator.verify_runtime_admission_blocked());
    }
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
fn u03_published_pointer_no_journal_reports_verified_app_without_runtime_admission() {
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
    assert!(view.can_complete_app);
    assert!(!view.can_recover_operation && !view.can_start_upgrade);
    assert!(coordinator.verify_runtime_admission_blocked());
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

#[test]
#[serial_test::serial]
fn u03_app_completion_requires_actual_fields_and_preserves_reliable_detached_mode() {
    let mut missed = Vec::new();
    for saved_mode in ["direct", "proxy"] {
        let f = Fixture::new();
        publish_resume_fixture(&f);
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        let config = serde_json::json!({"env":{
            "ANTHROPIC_AUTH_TOKEN":"synthetic-credential",
            "ANTHROPIC_BASE_URL":"https://example.invalid/v1",
            "ANTHROPIC_MODEL":"synthetic-model"
        }});
        for id in ["synthetic-ready", "synthetic-route"] {
            db.save_provider(
                "claude",
                &crate::provider::Provider::with_id(
                    id.into(),
                    "synthetic".into(),
                    if id == "synthetic-route" {
                        serde_json::json!({"env":{
                            "ANTHROPIC_AUTH_TOKEN":"synthetic-route-credential",
                            "ANTHROPIC_MODEL":"synthetic-route-model"
                        }})
                    } else {
                        config.clone()
                    },
                    None,
                ),
            )
            .unwrap();
        }
        db.set_current_provider("claude", "synthetic-ready")
            .unwrap();
        drop(db);
        f.write_settings(&crate::settings::AppSettings {
            current_provider_claude: Some("synthetic-ready".into()),
            ..Default::default()
        });
        let path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let state = format!(
            r#"{{"version":1,"apps":{{"claude":{{"mode":"{saved_mode}","proxy_route":"synthetic-route","attached":false}},"codex":{{"mode":"future-mode","pending":{{"op":"future-op","opaque":true}}}}}}}}"#
        );
        f.write_raw_mode(state.as_bytes());
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
        let app = crate::app_config::AppType::Claude;
        let view = coordinator.review_upgrade_app(&token, &app).unwrap();
        assert_eq!(
            view.saved_mode.as_ref().map(|mode| match mode {
                crate::mode::state::Mode::Direct => "direct",
                crate::mode::state::Mode::Proxy => "proxy",
            }),
            Some(saved_mode)
        );
        assert_eq!(view.has_pending_operation, Some(false));
        assert_eq!(view.pointer_consistent, Some(true));
        if view.stored_fields_match != Some(true) || !view.can_complete_app {
            missed.push(format!(
                "{saved_mode}: actual field proof/completion unavailable"
            ));
        }
        let peer = coordinator
            .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
            .unwrap();
        assert!(!peer.can_complete_app && !peer.can_recover_operation);
        assert_eq!(snapshot(f.home.path()), before);
        if saved_mode == "proxy" {
            f.write_raw_mode(
                state
                    .replace("synthetic-route", "synthetic-missing")
                    .as_bytes(),
            );
            let missing_route = coordinator.review_upgrade_app(&token, &app).unwrap();
            assert!(!missing_route.can_complete_app);
            assert_ne!(view.revision, missing_route.revision);
            f.write_raw_mode(state.as_bytes());
        }
        let unknown_mode = state.replace(
            &format!("\"mode\":\"{saved_mode}\""),
            "\"mode\":\"future-mode\"",
        );
        f.write_raw_mode(unknown_mode.as_bytes());
        let unverified = coordinator.review_upgrade_app(&token, &app).unwrap();
        assert!(unverified.saved_mode.is_none());
        assert!(!unverified.can_complete_app);
        f.write_raw_mode(state.as_bytes());
        let mut changed = config.clone();
        changed["env"]["ANTHROPIC_MODEL"] = serde_json::json!("synthetic-external-model");
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        let drifted = coordinator.review_upgrade_app(&token, &app).unwrap();
        assert!(!drifted.can_complete_app);
        assert_ne!(view.revision, drifted.revision);
        assert!(coordinator.verify_runtime_admission_blocked());
        let native_bedrock = serde_json::json!({"env":{
            "CLAUDE_CODE_USE_BEDROCK":"1",
            "AWS_BEARER_TOKEN_BEDROCK":"synthetic-bedrock-credential"
        }});
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        db.save_provider(
            "claude",
            &crate::provider::Provider::with_id(
                "synthetic-ready".into(),
                "synthetic".into(),
                native_bedrock.clone(),
                None,
            ),
        )
        .unwrap();
        drop(db);
        for legacy in [
            serde_json::json!({"apiKey":"synthetic-bedrock-credential","env":{"CLAUDE_CODE_USE_BEDROCK":"1"}}),
            serde_json::json!({"apiKey":"synthetic-stale-credential","env":{"CLAUDE_CODE_USE_BEDROCK":"1","AWS_BEARER_TOKEN_BEDROCK":"synthetic-bedrock-credential"}}),
        ] {
            std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
            let unprojected = coordinator.review_upgrade_app(&token, &app).unwrap();
            assert_eq!(unprojected.stored_fields_match, Some(true));
            assert!(
                !unprojected.can_complete_app,
                "unprojected native Bedrock fields certified"
            );
        }
        std::fs::write(&path, serde_json::to_vec(&native_bedrock).unwrap()).unwrap();
        assert!(
            coordinator
                .review_upgrade_app(&token, &app)
                .unwrap()
                .can_complete_app
        );
    }
    assert!(missed.is_empty(), "{}", missed.join("; "));
}

#[test]
#[serial_test::serial]
fn u03_runtime_database_constructor_reuses_verified_checkpoint_and_original_session() {
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut upgrade =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = upgrade.view(&inspected).unwrap().review_token.unwrap();
    let id = upgrade
        .prepare_checkpoint(&mut inspected, &token)
        .unwrap()
        .checkpoint_id
        .unwrap();
    upgrade
        .publish_checkpoint(&mut inspected, &token, &id, &f.store)
        .unwrap();
    let session = upgrade.session_for_test();
    let checkpoint_bytes = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
    let db = Database::init_with_secrets(session.clone()).unwrap_or_else(|error| {
        panic!("original runtime database owner must reopen its authenticated target20: {error}")
    });
    assert!(std::sync::Arc::ptr_eq(&db.secrets, &session));
    let conn = db.conn.lock().unwrap();
    assert_eq!(Database::get_user_version(&conn).unwrap(), 20);
    assert_eq!(
        database::loongport_schema::read_stored_version(&conn).unwrap(),
        24
    );
    database::vault::check_identity(&conn, &f.vault).unwrap();
    crate::secrets::inventory::validate_database(&conn, &f.vault).unwrap();
    drop(conn);
    assert_eq!(
        std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
        checkpoint_bytes
    );
    assert_eq!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault)
            .unwrap()
            .as_deref(),
        Some(id.as_str())
    );
    assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    assert!(
        database::vault::preflight(&f.root.join(crate::config::DB_FILE_NAME)).is_err(),
        "ordinary locked preflight must retain its older-version refusal"
    );
}

#[test]
#[serial_test::serial]
fn u03_runtime_database_constructor_refuses_unverified_target_without_mutation() {
    for case in [
        "missing_checkpoint",
        "corrupt_checkpoint",
        "future_schema",
        "generation_pending",
    ] {
        let f = Fixture::new();
        let mut inspected = inspect(&f.root, &f.device).unwrap();
        let mut upgrade =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = upgrade.view(&inspected).unwrap().review_token.unwrap();
        let id = upgrade
            .prepare_checkpoint(&mut inspected, &token)
            .unwrap()
            .checkpoint_id
            .unwrap();
        upgrade
            .publish_checkpoint(&mut inspected, &token, &id, &f.store)
            .unwrap();
        match case {
            "missing_checkpoint" => {
                std::fs::remove_file(f.device.root().join(checkpoint::FILE)).unwrap()
            }
            "corrupt_checkpoint" => std::fs::write(
                f.device.root().join(checkpoint::FILE),
                b"synthetic-invalid-checkpoint",
            )
            .unwrap(),
            "future_schema" => rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME))
                .unwrap()
                .pragma_update(None, "user_version", 21)
                .unwrap(),
            "generation_pending" => std::fs::write(
                f.root.join(crate::secrets::transition::INTENT),
                b"synthetic-pending-generation",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let before = snapshot(f.home.path());
        assert!(
            Database::init_with_secrets(upgrade.session_for_test()).is_err(),
            "{case}"
        );
        assert_eq!(snapshot(f.home.path()), before, "{case}");
    }
}

#[test]
#[serial_test::serial]
fn u03_native_proven_app_switches_while_future_peer_stays_blocked_and_sync_paused() {
    use crate::app_config::AppType;
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut upgrade =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = upgrade.view(&inspected).unwrap().review_token.unwrap();
    let id = upgrade
        .prepare_checkpoint(&mut inspected, &token)
        .unwrap()
        .checkpoint_id
        .unwrap();
    upgrade
        .publish_checkpoint(&mut inspected, &token, &id, &f.store)
        .unwrap();
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        upgrade.session_for_test(),
    );
    let config = |key: &str| {
        serde_json::json!({"env":{
            "ANTHROPIC_AUTH_TOKEN": format!("synthetic-{key}-credential"),
            "ANTHROPIC_BASE_URL": format!("https://{key}.example.invalid/v1"),
            "ANTHROPIC_MODEL": format!("synthetic-{key}-model")
        }})
    };
    for key in ["a", "b"] {
        db.save_provider(
            "claude",
            &crate::provider::Provider::with_id(key.into(), "synthetic".into(), config(key), None),
        )
        .unwrap();
    }
    db.set_current_provider("claude", "a").unwrap();
    f.write_settings(&crate::settings::AppSettings {
        current_provider_claude: Some("a".into()),
        ..Default::default()
    });
    let path = crate::config::get_claude_settings_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut initial = config("a");
    initial["unowned"] = serde_json::json!({"keep":true});
    std::fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
    f.write_raw_mode(br#"{"version":1,"apps":{"claude":{"mode":"direct","attached":false},"codex":{"mode":"future-mode","pending":{"op":"future-op","opaque":true,"large":123456789012345678901234567890}}}}"#);
    let original_checkpoint = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
    let opaque_peer = || {
        let file = crate::secrets::owned_file::DeviceFile::registered(
            crate::secrets::owned_file::DEVICE_STATE_FILE,
        )
        .unwrap();
        let plain = file
            .decode(&f.vault, &std::fs::read(f.device.state_path()).unwrap())
            .unwrap();
        let root: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_slice(&plain).unwrap();
        let apps: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_str(root["apps"].get()).unwrap();
        apps["codex"].get().to_owned()
    };
    let original_peer = opaque_peer();
    crate::settings::unlock_settings(upgrade.session_for_test()).unwrap();
    let state = crate::store::AppState::new(std::sync::Arc::new(db)).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &state.db.secrets,
        &upgrade.session_for_test()
    ));
    let result = crate::services::provider::ProviderService::switch(&state, AppType::Claude, "b");
    assert!(
        result.is_ok(),
        "original native Claude write must not be blocked by its peer: {:?}",
        result.err()
    );
    let native: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(native["env"], config("b")["env"]);
    assert_eq!(native["unowned"], serde_json::json!({"keep":true}));
    assert_eq!(
        state.db.get_current_provider("claude").unwrap().as_deref(),
        Some("b")
    );
    assert_eq!(
        crate::settings::get_current_provider_ready(&AppType::Claude)
            .unwrap()
            .as_deref(),
        Some("b")
    );
    assert!(
        crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &AppType::Claude)
            .is_ok()
    );
    assert!(
        crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &AppType::Codex)
            .is_err()
    );
    assert_eq!(
        std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
        original_checkpoint
    );
    assert_eq!(
        checkpoint::verified_database_id(&f.root, &f.device, &f.vault)
            .unwrap()
            .as_deref(),
        Some(id.as_str())
    );
    fn assert_paused<T>(result: Result<T, AppError>) {
        assert!(matches!(result, Err(AppError::Config(code)) if code == "upgrade.sync_paused"));
    }
    assert_paused(checkpoint::ensure_sync_admitted(&f.device));
    assert_eq!(opaque_peer(), original_peer);
    let before = snapshot(f.home.path());
    let mut dav = crate::settings::WebDavSyncSettings::default();
    let mut s3 = crate::settings::S3SyncSettings::default();
    assert_paused(crate::rt::block_on(crate::services::webdav_sync::upload(
        &state.db, &mut dav,
    )));
    assert_paused(crate::rt::block_on(crate::services::s3_sync::upload(
        &state.db, &mut s3,
    )));
    assert_paused(crate::rt::block_on(
        crate::services::webdav_sync::fetch_snapshot(&dav),
    ));
    assert_paused(crate::rt::block_on(
        crate::services::s3_sync::fetch_snapshot(&s3),
    ));
    assert_eq!(snapshot(f.home.path()), before);
}

#[test]
#[serial_test::serial]
fn u03_checkpoint_native_admission_refuses_unproven_app_without_mutation() {
    for case in [
        "native_drift",
        "pointer_drift",
        "missing_mode",
        "future_mode",
        "future_root",
        "checkpoint_corrupt",
        "generation_pending",
    ] {
        use crate::app_config::AppType;
        let f = Fixture::new();
        let mut inspected = inspect(&f.root, &f.device).unwrap();
        let mut upgrade =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = upgrade.view(&inspected).unwrap().review_token.unwrap();
        let id = upgrade
            .prepare_checkpoint(&mut inspected, &token)
            .unwrap()
            .checkpoint_id
            .unwrap();
        upgrade
            .publish_checkpoint(&mut inspected, &token, &id, &f.store)
            .unwrap();
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            upgrade.session_for_test(),
        );
        let config = |key: &str| {
            serde_json::json!({"env":{
                "ANTHROPIC_AUTH_TOKEN": format!("synthetic-{key}-credential"),
                "ANTHROPIC_BASE_URL": format!("https://{key}.example.invalid/v1"),
                "ANTHROPIC_MODEL": format!("synthetic-{key}-model")
            }})
        };
        for key in ["a", "b"] {
            db.save_provider(
                "claude",
                &crate::provider::Provider::with_id(
                    key.into(),
                    "synthetic".into(),
                    config(key),
                    None,
                ),
            )
            .unwrap();
        }
        db.set_current_provider("claude", "a").unwrap();
        f.write_settings(&crate::settings::AppSettings {
            current_provider_claude: Some("a".into()),
            ..Default::default()
        });
        let path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut initial = config("a");
        initial["unowned"] = serde_json::json!({"keep":true});
        std::fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
        f.write_raw_mode(br#"{"version":1,"apps":{"claude":{"mode":"direct","attached":false},"codex":{"mode":"future-mode","pending":{"op":"future-op","opaque":true,"large":123456789012345678901234567890}}}}"#);
        crate::settings::unlock_settings(upgrade.session_for_test()).unwrap();
        let state = crate::store::AppState::new(std::sync::Arc::new(db)).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &state.db.secrets,
            &upgrade.session_for_test()
        ));

        match case {
            "native_drift" => {
                let mut value = initial.clone();
                value["env"]["ANTHROPIC_AUTH_TOKEN"] = serde_json::json!("synthetic-external-change");
                std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "pointer_drift" => f.write_settings(&crate::settings::AppSettings {
                current_provider_claude: Some("b".into()),
                ..Default::default()
            }),
            "missing_mode" => f.write_raw_mode(br#"{"version":1,"apps":{}}"#),
            "future_mode" => f.write_raw_mode(br#"{"version":1,"apps":{"claude":{"mode":"future-mode","opaque":true}}}"#),
            "future_root" => f.write_raw_mode(br#"{"version":1,"future":true,"apps":{"claude":{"mode":"direct","attached":false}}}"#),
            "checkpoint_corrupt" => std::fs::write(f.device.root().join(checkpoint::FILE), b"synthetic-invalid-envelope").unwrap(),
            "generation_pending" => std::fs::write(f.root.join(crate::secrets::transition::INTENT), b"synthetic-pending-generation").unwrap(),
            _ => unreachable!(),
        }
        let before = snapshot(f.home.path());
        assert!(
            crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &AppType::Claude)
                .is_err(),
            "{case} must retain app admission block"
        );
        assert_eq!(
            snapshot(f.home.path()),
            before,
            "{case} must remain read-only"
        );
    }
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn u03_native_admission_rejects_same_path_database_replacement_with_old_connection() {
    use crate::app_config::AppType;
    let f = Fixture::new();
    let mut inspected = inspect(&f.root, &f.device).unwrap();
    let mut upgrade =
        AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None).unwrap();
    let token = upgrade.view(&inspected).unwrap().review_token.unwrap();
    let id = upgrade
        .prepare_checkpoint(&mut inspected, &token)
        .unwrap()
        .checkpoint_id
        .unwrap();
    upgrade
        .publish_checkpoint(&mut inspected, &token, &id, &f.store)
        .unwrap();
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        upgrade.session_for_test(),
    );
    let config = |key: &str| {
        serde_json::json!({"env":{
            "ANTHROPIC_AUTH_TOKEN": format!("synthetic-{key}-credential"),
            "ANTHROPIC_BASE_URL": format!("https://{key}.example.invalid/v1"),
            "ANTHROPIC_MODEL": format!("synthetic-{key}-model")
        }})
    };
    for key in ["a", "b"] {
        db.save_provider(
            "claude",
            &crate::provider::Provider::with_id(key.into(), "synthetic".into(), config(key), None),
        )
        .unwrap();
    }
    db.set_current_provider("claude", "a").unwrap();
    f.write_settings(&crate::settings::AppSettings {
        current_provider_claude: Some("a".into()),
        ..Default::default()
    });
    let path = crate::config::get_claude_settings_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut initial = config("a");
    initial["unowned"] = serde_json::json!({"keep":true});
    std::fs::write(&path, serde_json::to_vec(&initial).unwrap()).unwrap();
    f.write_raw_mode(br#"{"version":1,"apps":{"claude":{"mode":"direct","attached":false},"codex":{"mode":"future-mode","pending":{"op":"future-op","opaque":true,"large":123456789012345678901234567890}}}}"#);
    crate::settings::unlock_settings(upgrade.session_for_test()).unwrap();
    let state = crate::store::AppState::new(std::sync::Arc::new(db)).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &state.db.secrets,
        &upgrade.session_for_test()
    ));

    let database_path = f.root.join(crate::config::DB_FILE_NAME);
    let bytes = std::fs::read(&database_path).unwrap();
    let moved = f.root.join("synthetic-original-database.db");
    std::fs::rename(&database_path, &moved).unwrap();
    std::fs::write(&database_path, &bytes).unwrap();
    let replacement = Database::from_connection(
        rusqlite::Connection::open(&database_path).unwrap(),
        upgrade.session_for_test(),
    );
    crate::database::vault::check_identity(&replacement.conn.lock().unwrap(), &f.vault).unwrap();
    assert_eq!(
        Database::get_user_version(&replacement.conn.lock().unwrap()).unwrap(),
        20
    );
    drop(replacement);
    let before = snapshot(f.home.path());
    assert!(
        crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &AppType::Claude)
            .is_err(),
        "same-path same-Vault replacement cannot authorize the old connection"
    );
    assert_eq!(snapshot(f.home.path()), before);
}

#[test]
#[serial_test::serial]
fn u03_gemini_native_completion_uses_original_projection_and_keeps_peer_isolated() {
    for mode in ["direct", "proxy"] {
        for official in [false, true] {
            let f = Fixture::new();
            publish_resume_fixture(&f);
            let db = Database::from_connection(
                rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
                session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
            );
            let mut row = crate::provider::Provider::with_id(
                "synthetic-gemini".into(),
                "synthetic".into(),
                serde_json::json!({"env": {
                    "GEMINI_API_KEY": "synthetic-key",
                    "GOOGLE_GEMINI_BASE_URL": "https://example.invalid",
                    "GEMINI_MODEL": "synthetic-model"
                }, "config": {"model": {"name": "synthetic-model"}}}),
                None,
            );
            if official {
                row.category = Some("official".into());
            }
            let projection = crate::services::provider::gemini_direct::projection(&row).unwrap();
            db.save_provider("gemini", &row).unwrap();
            db.set_current_provider("gemini", &row.id).unwrap();
            drop(db);
            f.write_settings(&crate::settings::AppSettings {
                current_provider_gemini: Some(row.id.clone()),
                ..Default::default()
            });
            f.write_raw_mode(format!(r#"{{"version":1,"apps":{{"gemini":{{"mode":"{mode}","attached":false,"proxy_route":"synthetic-gemini"}},"codex":{{"mode":"future-mode"}}}}}}"#).as_bytes());
            use crate::live::patch::LivePatch;
            let env_path = crate::gemini_config::get_gemini_env_path();
            let config_path = crate::gemini_config::get_gemini_settings_path();
            std::fs::create_dir_all(env_path.parent().unwrap()).unwrap();
            let env = projection
                .env_patch()
                .apply(&env_path, Some(b"# keep\nUSER_OPTION=keep\n"))
                .unwrap();
            let config = projection
                .settings_patch()
                .apply(&config_path, Some(br#"{"ui":{"theme":"synthetic"}}"#))
                .unwrap();
            std::fs::write(&env_path, &env).unwrap();
            std::fs::write(&config_path, &config).unwrap();
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
                .review_upgrade_app(&token, &crate::app_config::AppType::Gemini)
                .unwrap();
            assert_eq!(view.stored_fields_match, Some(true));
            assert!(view.can_complete_app, "mode={mode}, official={official}");
            assert_eq!(snapshot(f.home.path()), before);
            assert!(
                !coordinator
                    .review_upgrade_app(&token, &crate::app_config::AppType::Codex)
                    .unwrap()
                    .can_complete_app
            );
            for suffix in [
                "GOOGLE_CLOUD_PROJECT=unexpected\n",
                "GEMINI_API_KEY=${FROM_ENV}\n",
                "GEMINI_API_KEY=PROXY_MANAGED\n",
            ] {
                let mut changed = env.clone();
                changed.extend_from_slice(suffix.as_bytes());
                std::fs::write(&env_path, changed).unwrap();
                let before = snapshot(f.home.path());
                let changed = coordinator
                    .review_upgrade_app(&token, &crate::app_config::AppType::Gemini)
                    .unwrap();
                assert!(!changed.can_complete_app);
                assert_ne!(changed.revision, view.revision);
                assert_eq!(snapshot(f.home.path()), before);
            }
            std::fs::write(&env_path, &env).unwrap();
            std::fs::write(
                &config_path,
                br#"{"security":{"auth":{"selectedType":"gemini-api-key"}},"model":"unsupported"}"#,
            )
            .unwrap();
            assert!(
                !coordinator
                    .review_upgrade_app(&token, &crate::app_config::AppType::Gemini)
                    .unwrap()
                    .can_complete_app
            );
        }
    }
}

#[test]
fn u03_gemini_native_completion_does_not_treat_malformed_model_as_absent() {
    let row = crate::provider::Provider::with_id(
        "synthetic".into(),
        "synthetic".into(),
        serde_json::json!({"env":{"GEMINI_API_KEY":"synthetic-key"}}),
        None,
    );
    let live = serde_json::json!({"env":{"GEMINI_API_KEY":"synthetic-key"}, "config":{"security":{"auth":{"selectedType":"gemini-api-key"}},"model":"unsupported"}});
    assert_ne!(
        super::projection_review::native_completion_match(
            &crate::app_config::AppType::Gemini,
            Some(&row),
            &live
        ),
        Some(true)
    );
}

#[test]
#[serial_test::serial]
fn u03_gemini_retained_checkpoint_admits_actual_native_owner_and_keeps_sync_paused() {
    use crate::app_config::AppType;
    use crate::live::patch::LivePatch;
    for saved_mode in ["direct", "proxy"] {
        let f = Fixture::new();
        publish_resume_fixture(&f);
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        for id in ["a", "b"] {
            db.save_provider("gemini", &crate::provider::Provider::with_id(id.into(), "synthetic".into(), serde_json::json!({"env":{"GEMINI_API_KEY":format!("synthetic-{id}"),"GOOGLE_GEMINI_BASE_URL":format!("https://{id}.example.invalid")},"config":{"model":{"name":format!("synthetic-{id}")}}}), None)).unwrap();
        }
        let row = db.get_provider_by_id("a", "gemini").unwrap().unwrap();
        db.set_current_provider("gemini", "a").unwrap();
        drop(db);
        f.write_settings(&crate::settings::AppSettings {
            current_provider_gemini: Some("a".into()),
            ..Default::default()
        });
        f.write_raw_mode(format!(r#"{{"version":1,"apps":{{"gemini":{{"mode":"{saved_mode}","attached":false,"proxy_route":"a"}},"codex":{{"mode":"future-mode","opaque":900719925474099312345}}}}}}"#).as_bytes());
        let env_path = crate::gemini_config::get_gemini_env_path();
        let config_path = crate::gemini_config::get_gemini_settings_path();
        std::fs::create_dir_all(env_path.parent().unwrap()).unwrap();
        let projection = crate::services::provider::gemini_direct::projection(&row).unwrap();
        std::fs::write(
            &env_path,
            projection
                .env_patch()
                .apply(&env_path, Some(b"# synthetic-user\nUSER_OPTION=keep\n"))
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            &config_path,
            projection
                .settings_patch()
                .apply(&config_path, Some(br#"{"ui":{"theme":"synthetic-user"}}"#))
                .unwrap(),
        )
        .unwrap();
        let inspected = inspect(&f.root, &f.device).unwrap();
        let review =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = review.view(&inspected).unwrap().review_token.unwrap();
        let session = review.runtime_session(&inspected, &token).unwrap();
        crate::settings::unlock_settings(session.clone()).unwrap();
        let db = std::sync::Arc::new(Database::init_with_secrets(session.clone()).unwrap());
        assert!(std::sync::Arc::ptr_eq(&db.secrets, &session));
        let state = crate::store::AppState::new(db).unwrap();
        let before = snapshot(f.home.path());
        drop(
            crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, &AppType::Gemini)
                .unwrap(),
        );
        assert_eq!(snapshot(f.home.path()), before);
        if saved_mode == "direct" {
            crate::services::ProviderService::switch(&state, AppType::Gemini, "b").unwrap();
            assert_eq!(
                crate::settings::get_current_provider_ready(&AppType::Gemini)
                    .unwrap()
                    .as_deref(),
                Some("b")
            );
            assert_eq!(
                state.db.get_current_provider("gemini").unwrap().as_deref(),
                Some("b")
            );
            assert!(std::fs::read_to_string(&env_path)
                .unwrap()
                .contains("GEMINI_API_KEY=synthetic-b"));
            assert!(std::fs::read_to_string(&env_path)
                .unwrap()
                .contains("USER_OPTION=keep"));
            let config: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
            assert_eq!(
                config
                    .pointer("/model/name")
                    .and_then(serde_json::Value::as_str),
                Some("synthetic-b")
            );
            assert_eq!(
                config
                    .pointer("/ui/theme")
                    .and_then(serde_json::Value::as_str),
                Some("synthetic-user")
            );
            assert!(
                review
                    .review_app(&inspected, &token, &AppType::Gemini)
                    .unwrap()
                    .can_complete_app
            );
        }
        let mode =
            crate::mode::state::mode_state(&f.device, &session.read().unwrap(), "gemini").unwrap();
        assert_eq!(
            mode.mode,
            Some(if saved_mode == "direct" {
                crate::mode::state::Mode::Direct
            } else {
                crate::mode::state::Mode::Proxy
            })
        );
        assert!(!mode.attached);
        assert_eq!(mode.proxy_route.as_deref(), Some("a"));
        assert!(
            crate::mode::state::pending(&f.device, &session.read().unwrap(), "gemini")
                .unwrap()
                .is_none()
        );
        let before = snapshot(f.home.path());
        assert!(crate::mode::operation::AppWrite::begin_mode(
            &state.proxy_service,
            &AppType::Codex
        )
        .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
        assert_eq!(
            checkpoint::verified_database_id(&f.root, &f.device, &session.read().unwrap()).unwrap(),
            review.view(&inspected).unwrap().checkpoint_id
        );
    }
}

#[test]
#[serial_test::serial]
fn u03_grok_retained_checkpoint_uses_original_native_and_written_owners() {
    use crate::app_config::AppType;
    use crate::mode::state::{self, Mode};
    let config = |id: &str| {
        format!("[models]\ndefault = '{id}'\n[model.{id}]\nmodel = 'synthetic-{id}'\nname = 'Synthetic {id}'\nbase_url = 'https://{id}.example.invalid/v1'\napi_key = 'synthetic-key-{id}'\napi_backend = 'responses'\ncontext_window = 200000\n")
    };
    for scenario in ["direct", "proxy", "missing"] {
        let f = Fixture::new();
        publish_resume_fixture(&f);
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        for id in ["a", "b"] {
            db.save_provider(
                "grokbuild",
                &crate::provider::Provider::with_id(
                    id.into(),
                    format!("Synthetic {id}"),
                    serde_json::json!({"config":config(id)}),
                    None,
                ),
            )
            .unwrap();
        }
        if scenario != "missing" {
            db.set_current_provider("grokbuild", "a").unwrap();
        }
        drop(db);
        f.write_settings(&crate::settings::AppSettings {
            current_provider_grokbuild: (scenario != "missing").then(|| "a".into()),
            ..Default::default()
        });
        let saved_mode = if scenario == "proxy" {
            "proxy"
        } else {
            "direct"
        };
        f.write_raw_mode(format!(r#"{{"version":1,"apps":{{"grokbuild":{{"mode":"{saved_mode}","attached":false,"proxy_route":"a"}},"codex":{{"mode":"future-mode","opaque":900719925474099312345}}}}}}"#).as_bytes());
        let path = crate::grok_config::get_grok_config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("# synthetic-user\n{}\n[ui]\ntheme = 'keep'\n[model.mine]\nmodel = 'synthetic-user'\napi_key = 'synthetic-user-key'\n[mcp_servers.local]\ncommand = 'synthetic'\n", config("a"))).unwrap();
        let inspected = inspect(&f.root, &f.device).unwrap();
        let review =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = review.view(&inspected).unwrap().review_token.unwrap();
        let session = review.runtime_session(&inspected, &token).unwrap();
        crate::settings::unlock_settings(session.clone()).unwrap();
        let db = std::sync::Arc::new(Database::init_with_secrets(session.clone()).unwrap());
        assert!(std::sync::Arc::ptr_eq(&db.secrets, &session));
        let runtime = crate::store::AppState::new(db).unwrap();
        if scenario == "missing" {
            let view = review
                .review_app(&inspected, &token, &AppType::GrokBuild)
                .unwrap();
            assert!(view.can_choose_provider);
            assert_eq!(
                view.keep_files_providers
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["a"]
            );
            let bytes = std::fs::read(&path).unwrap();
            let chosen = review
                .select_provider(
                    &inspected,
                    &token,
                    &AppType::GrokBuild,
                    &view.revision,
                    "a",
                    &runtime,
                )
                .unwrap();
            assert!(chosen.can_complete_app);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        assert!(
            review
                .review_app(&inspected, &token, &AppType::GrokBuild)
                .unwrap()
                .can_complete_app
        );
        let before = snapshot(f.home.path());
        drop(
            crate::mode::operation::AppWrite::begin_mode(
                &runtime.proxy_service,
                &AppType::GrokBuild,
            )
            .unwrap(),
        );
        assert_eq!(snapshot(f.home.path()), before);
        state::update_app(&f.device, &session.read().unwrap(), "grokbuild", |entry| {
            entry.written =
                Some(serde_json::from_value(serde_json::json!({"codex":{"version":1}})).unwrap());
            Ok(())
        })
        .unwrap();
        let foreign_written = snapshot(f.home.path());
        assert!(
            !review
                .review_app(&inspected, &token, &AppType::GrokBuild)
                .unwrap()
                .can_complete_app
        );
        assert!(crate::mode::operation::AppWrite::begin_mode(
            &runtime.proxy_service,
            &AppType::GrokBuild
        )
        .is_err());
        assert_eq!(snapshot(f.home.path()), foreign_written);
        state::update_app(&f.device, &session.read().unwrap(), "grokbuild", |entry| {
            entry.written = None;
            Ok(())
        })
        .unwrap();
        if saved_mode == "direct" {
            crate::services::ProviderService::switch(&runtime, AppType::GrokBuild, "b").unwrap();
            let native = std::fs::read(&path).unwrap();
            let mut doc = std::str::from_utf8(&native)
                .unwrap()
                .parse::<toml_edit::DocumentMut>()
                .unwrap();
            assert_eq!(doc["models"]["default"].as_str(), Some("b"));
            assert!(doc["model"].get("a").is_none());
            assert_eq!(
                doc["model"]["mine"]["api_key"].as_str(),
                Some("synthetic-user-key")
            );
            assert_eq!(doc["ui"]["theme"].as_str(), Some("keep"));
            assert_eq!(
                doc["mcp_servers"]["local"]["command"].as_str(),
                Some("synthetic")
            );
            assert_eq!(
                runtime
                    .db
                    .get_current_provider("grokbuild")
                    .unwrap()
                    .as_deref(),
                Some("b")
            );
            assert_eq!(
                crate::settings::get_current_provider_ready(&AppType::GrokBuild)
                    .unwrap()
                    .as_deref(),
                Some("b")
            );
            assert_eq!(
                state::written(&f.device, &session.read().unwrap(), "grokbuild")
                    .unwrap()
                    .unwrap()
                    .tables,
                vec!["b"]
            );
            assert!(
                review
                    .review_app(&inspected, &token, &AppType::GrokBuild)
                    .unwrap()
                    .can_complete_app
            );

            // Whole-table proof must reject an obsolete credential beside the
            // otherwise correct row, without rewriting the native input.
            doc["model"]["b"]["env_key"] = toml_edit::value("SYNTHETIC_OLD_KEY");
            std::fs::write(&path, doc.to_string()).unwrap();
            let changed = snapshot(f.home.path());
            assert!(
                !review
                    .review_app(&inspected, &token, &AppType::GrokBuild)
                    .unwrap()
                    .can_complete_app
            );
            assert!(crate::mode::operation::AppWrite::begin_mode(
                &runtime.proxy_service,
                &AppType::GrokBuild
            )
            .is_err());
            assert_eq!(snapshot(f.home.path()), changed);
            std::fs::write(&path, &native).unwrap();

            // The authoritative original Written owner also requires removal
            // of retired tables, even when models.default already names b.
            let old = config("a").parse::<toml_edit::DocumentMut>().unwrap();
            doc = std::str::from_utf8(&native).unwrap().parse().unwrap();
            doc["model"]
                .as_table_like_mut()
                .unwrap()
                .insert("a", old["model"]["a"].clone());
            std::fs::write(&path, doc.to_string()).unwrap();
            state::update_app(&f.device, &session.read().unwrap(), "grokbuild", |entry| {
                entry.written.as_mut().unwrap().tables.push("a".into());
                Ok(())
            })
            .unwrap();
            let changed = snapshot(f.home.path());
            let stale = review
                .review_app(&inspected, &token, &AppType::GrokBuild)
                .unwrap();
            assert_eq!(stale.stored_fields_match, Some(true));
            assert!(!stale.can_complete_app);
            assert_eq!(snapshot(f.home.path()), changed);
            std::fs::write(&path, &native).unwrap();
            assert!(
                review
                    .review_app(&inspected, &token, &AppType::GrokBuild)
                    .unwrap()
                    .can_complete_app
            );

            // A later row refresh is preserved and cannot be admitted as if
            // its credentials had already been published to the native file.
            let mut row = runtime
                .db
                .get_provider_by_id("b", "grokbuild")
                .unwrap()
                .unwrap();
            row.settings_config["config"] = serde_json::json!(
                config("b").replace("synthetic-key-b", "synthetic-refreshed-key")
            );
            runtime.db.save_provider("grokbuild", &row).unwrap();
            // A WAL reader can move SQLite's transient read mark after this
            // explicit row write. Compare every durable file (including WAL),
            // retain the index's presence, and separately verify logical DB data.
            let shm = f
                .root
                .join(format!("{}-shm", crate::config::DB_FILE_NAME))
                .strip_prefix(f.home.path())
                .unwrap()
                .to_path_buf();
            let stable_snapshot = || {
                use sha2::Digest;
                snapshot(f.home.path())
                    .into_iter()
                    .map(|(path, bytes)| {
                        let digest = if path == shm {
                            "sqlite-wal-index".to_owned()
                        } else {
                            hex::encode(sha2::Sha256::digest(bytes))
                        };
                        (path, digest)
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let changed = stable_snapshot();
            let database = Database::content_digest(&runtime.db.conn.lock().unwrap()).unwrap();
            assert!(
                !review
                    .review_app(&inspected, &token, &AppType::GrokBuild)
                    .unwrap()
                    .can_complete_app
            );
            assert!(crate::mode::operation::AppWrite::begin_mode(
                &runtime.proxy_service,
                &AppType::GrokBuild
            )
            .is_err());
            assert_eq!(stable_snapshot(), changed);
            assert_eq!(
                Database::content_digest(&runtime.db.conn.lock().unwrap()).unwrap(),
                database
            );
            assert_eq!(
                runtime
                    .db
                    .get_provider_by_id("b", "grokbuild")
                    .unwrap()
                    .unwrap()
                    .settings_config,
                row.settings_config
            );
            assert_eq!(std::fs::read(&path).unwrap(), native);
        }
        let mode = state::mode_state(&f.device, &session.read().unwrap(), "grokbuild").unwrap();
        assert_eq!(
            mode.mode,
            Some(if saved_mode == "proxy" {
                Mode::Proxy
            } else {
                Mode::Direct
            })
        );
        assert!(!mode.attached);
        assert_eq!(mode.proxy_route.as_deref(), Some("a"));
        assert!(
            state::pending(&f.device, &session.read().unwrap(), "grokbuild")
                .unwrap()
                .is_none()
        );
        let before = snapshot(f.home.path());
        assert!(crate::mode::operation::AppWrite::begin_mode(
            &runtime.proxy_service,
            &AppType::Codex
        )
        .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
        assert_eq!(
            checkpoint::verified_database_id(&f.root, &f.device, &session.read().unwrap()).unwrap(),
            review.view(&inspected).unwrap().checkpoint_id
        );
    }
}

#[test]
#[serial_test::serial]
fn u03_keep_files_provider_choice_uses_original_pending_and_settings_owner() {
    use crate::app_config::AppType;
    for (source, failure) in [
        ("missing", None),
        ("conflict", None),
        ("missing", Some("target-before-pointer")),
        ("conflict", Some("target-before-pointer")),
        ("missing", Some("row-drift")),
        ("missing", Some("credentials-after-pointer")),
        ("multiple", None),
        ("multiple", Some("target-before-pointer")),
        ("preserved", None),
    ] {
        let f = Fixture::new();
        publish_resume_fixture(&f);
        let db = Database::from_connection(
            rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        let config = serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-key-a","ANTHROPIC_BASE_URL":"https://a.example.invalid"}});
        for id in ["a", "b"] {
            db.save_provider(
                "claude",
                &crate::provider::Provider::with_id(
                    id.into(),
                    format!("Synthetic {id}"),
                    if id == "a" {
                        config.clone()
                    } else {
                        serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-key-b"}})
                    },
                    None,
                ),
            )
            .unwrap();
        }
        if source == "conflict" {
            db.set_current_provider("claude", "b").unwrap();
        } else if source == "multiple" {
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE providers SET is_current=1 WHERE app_type='claude'",
                    [],
                )
                .unwrap();
        } else if source == "preserved" {
            db.set_current_provider("claude", "a").unwrap();
        }
        drop(db);
        f.write_settings(&crate::settings::AppSettings {
            current_provider_claude: (source == "conflict").then(|| "missing-row".into()),
            ..Default::default()
        });
        f.write_raw_mode(br#"{"version":1,"apps":{"claude":{"mode":"direct"},"codex":{"mode":"future-mode","opaque":900719925474099312345}}}"#);
        let path = crate::config::get_claude_settings_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let inspected = inspect(&f.root, &f.device).unwrap();
        let review =
            AuthenticatedUpgrade::authenticate(&f.root, &f.device, &inspected, &f.store, None)
                .unwrap();
        let token = review.view(&inspected).unwrap().review_token.unwrap();
        let session = review.runtime_session(&inspected, &token).unwrap();
        crate::settings::unlock_settings(session.clone()).unwrap();
        let state = crate::store::AppState::new(std::sync::Arc::new(
            Database::init_with_secrets(session.clone()).unwrap(),
        ))
        .unwrap();
        let view = review
            .review_app(&inspected, &token, &AppType::Claude)
            .unwrap();
        assert_eq!(
            view.direct_provider_resolution,
            if source == "multiple" {
                "conflict"
            } else {
                source
            }
        );
        if source == "preserved" {
            assert_eq!(view.retained_provider_id.as_deref(), Some("a"));
        }
        assert!(view.can_choose_provider);
        assert_eq!(
            view.keep_files_providers
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        if source == "missing" && failure.is_none() {
            let coordinator = crate::secrets::startup::StartupCoordinator::new(
                f.root.clone(),
                inspect(&f.root, &f.device).unwrap(),
            );
            let before = snapshot(f.home.path());
            assert_eq!(
                coordinator
                    .select_upgrade_provider(&token, &AppType::Claude, &view.revision, "a", &state)
                    .err()
                    .as_deref(),
                Some("secret.locked")
            );
            assert_eq!(snapshot(f.home.path()), before);
        }
        let before = snapshot(f.home.path());
        assert!(review
            .select_provider(&inspected, &token, &AppType::Claude, "stale", "a", &state)
            .is_err());
        assert!(review
            .select_provider(
                &inspected,
                &token,
                &AppType::Claude,
                &view.revision,
                "b",
                &state
            )
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        // An unrelated owned setting changes after the runtime cache was loaded.
        // Pointer publication must refresh that original cache without losing it.
        let settings_path = crate::settings::settings_path();
        let mut document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        document["language"] = serde_json::json!("ja");
        std::fs::write(&settings_path, serde_json::to_vec(&document).unwrap()).unwrap();
        let native = std::fs::read(&path).unwrap();
        if failure == Some("target-before-pointer") {
            crate::mode::operation::failpoint::crash_at(Some("upgrade:provider_target"));
        } else if failure == Some("credentials-after-pointer") {
            crate::mode::operation::failpoint::crash_at(Some("target"));
        } else if failure == Some("row-drift") {
            let path = f.root.join(crate::config::DB_FILE_NAME);
            crate::mode::operation::failpoint::on_boundary(Some(Box::new(move |point| {
                if point == "upgrade:provider_target" {
                    rusqlite::Connection::open(&path).unwrap().execute(
                        "UPDATE providers SET name='Externally changed' WHERE app_type='claude' AND id='a'", [],
                    ).unwrap();
                }
            })));
        }
        let selected = review.select_provider(
            &inspected,
            &token,
            &AppType::Claude,
            &view.revision,
            "a",
            &state,
        );
        crate::mode::operation::failpoint::crash_at(None);
        crate::mode::operation::failpoint::on_boundary(None);
        if failure == Some("credentials-after-pointer") {
            assert!(selected.is_err());
            let mut refreshed = state.db.get_provider_by_id("a", "claude").unwrap().unwrap();
            refreshed.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"] =
                serde_json::json!("synthetic-refreshed-key");
            state.db.save_provider("claude", &refreshed).unwrap();
            let before = snapshot(f.home.path());
            let current = review
                .review_app(&inspected, &token, &AppType::Claude)
                .unwrap();
            assert!(!current.can_recover_operation);
            assert!(!current.can_complete_app);
            assert!(review
                .recover_app(&inspected, &token, &AppType::Claude, &current.revision)
                .is_err());
            assert_eq!(snapshot(f.home.path()), before);
            assert_eq!(std::fs::read(&path).unwrap(), native);
            assert_eq!(
                state
                    .db
                    .get_provider_by_id("a", "claude")
                    .unwrap()
                    .unwrap()
                    .settings_config,
                refreshed.settings_config
            );
            assert!(
                crate::mode::state::pending(&f.device, &session.read().unwrap(), "claude")
                    .unwrap()
                    .is_some()
            );
            continue;
        }
        if failure == Some("row-drift") {
            assert!(selected.is_err());
            assert_eq!(
                crate::settings::get_current_provider_ready(&AppType::Claude).unwrap(),
                None
            );
            assert_eq!(state.db.get_current_provider("claude").unwrap(), None);
            assert_eq!(std::fs::read(&path).unwrap(), native);
            assert!(
                crate::mode::state::pending(&f.device, &session.read().unwrap(), "claude")
                    .unwrap()
                    .is_some()
            );
            continue;
        }
        let result = if failure.is_some() {
            assert!(selected.is_err());
            let pending =
                crate::mode::state::pending(&f.device, &session.read().unwrap(), "claude")
                    .unwrap()
                    .unwrap();
            assert!(pending.published);
            assert!(pending.files.iter().all(|file| file.pre == file.planned));
            let pending_view = review
                .review_app(&inspected, &token, &AppType::Claude)
                .unwrap();
            assert!(pending_view.can_recover_operation);
            assert!(!pending_view.can_choose_provider);
            assert!(review
                .recover_app(&inspected, &token, &AppType::Claude, &view.revision)
                .is_err());
            review
                .recover_app_with_state(
                    &inspected,
                    &token,
                    &AppType::Claude,
                    &pending_view.revision,
                    &state,
                )
                .unwrap()
        } else {
            selected.unwrap()
        };
        assert!(result.can_complete_app);
        assert_eq!(
            crate::settings::get_settings().language.as_deref(),
            Some("ja")
        );
        assert!(
            crate::mode::state::load_app(&f.device, &session.read().unwrap(), "codex").is_err()
        );
        let raw = f
            .device
            .read_device(
                &session.read().unwrap(),
                &crate::secrets::owned_file::DeviceFile::registered(
                    crate::secrets::owned_file::DEVICE_STATE_FILE,
                )
                .unwrap(),
            )
            .unwrap()
            .unwrap();
        assert!(std::str::from_utf8(&raw)
            .unwrap()
            .contains("900719925474099312345"));
        assert_eq!(std::fs::read(&path).unwrap(), native);
        assert_eq!(
            crate::settings::get_current_provider_ready(&AppType::Claude)
                .unwrap()
                .as_deref(),
            Some("a")
        );
        assert_eq!(
            state.db.get_current_provider("claude").unwrap().as_deref(),
            Some("a")
        );
        assert!(
            crate::mode::state::pending(&f.device, &session.read().unwrap(), "claude")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            crate::mode::state::mode_state(&f.device, &session.read().unwrap(), "claude")
                .unwrap()
                .mode,
            Some(crate::mode::state::Mode::Direct)
        );
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    }
}
