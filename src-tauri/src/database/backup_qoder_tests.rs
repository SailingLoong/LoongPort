// Included only inside backup.rs::tests. These tests call the original import,
// backup/restore and encrypted sync-join owners, not the preservation helpers.
use crate::database::dao::qoder_campaign_operations as qoder_receipts;
use crate::services::provider::qoder_daily::{self as qoder, contract as qoder_contract};

fn qoder_state(db: &Database) -> qoder::State {
    db.get_provider_by_id("qoder-restore-fixture", "codex")
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .qoder_campaign_state
        .unwrap()
}

fn qoder_fixture(
    db: &Database,
    credential: &str,
    subject: &str,
    attempt: &str,
    start: i64,
) -> Result<qoder::Operation, AppError> {
    let mut provider = crate::provider::Provider::with_id(
        "qoder-restore-fixture".into(),
        "Synthetic Qoder restore fixture".into(),
        serde_json::json!({"env":{"OPENAI_API_KEY":credential}}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_sdk".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider)?;
    let provider = db.get_provider_by_id(&provider.id, "codex")?.unwrap();
    let generation = provider
        .meta
        .as_ref()
        .unwrap()
        .qoder_campaign_state
        .as_ref()
        .unwrap()
        .generation
        .clone();
    let operation = qoder::Operation {
        id: attempt.into(),
        generation,
        binding: qoder_contract::BoundCampaign::from_operation(
            qoder_contract::Identity {
                provider_id: provider.id.clone(),
                uid: subject.into(),
                region: qoder_contract::Region::International,
            },
            format!("synthetic-campaign-{start}"),
            format!("synthetic-key-{start}"),
            start,
            start + 100,
        )
        .expect("valid synthetic operation binding"),
        outcome: qoder_contract::Outcome::Unknown,
    };
    assert_eq!(
        db.save_qoder_operation(&provider, &operation)?,
        qoder::PersistOutcome::Applied
    );
    Ok(operation)
}

#[test]
fn qoder_import_keeps_local_receipt_and_normalizes_real_provider_generation() -> Result<(), AppError>
{
    for incoming_key in ["synthetic-current", "synthetic-changed"] {
        let db = Database::memory()?;
        qoder_fixture(
            &db,
            "synthetic-current",
            "local-subject",
            "local-attempt",
            100,
        )?;
        let before = qoder_state(&db);
        let expected_rows = qoder_receipts::read_rows(&*db.conn.lock()?)?;
        let incoming = same_vault_database(&db)?;
        qoder_fixture(
            &incoming,
            incoming_key,
            "foreign-subject",
            "foreign-attempt",
            100,
        )?;
        let foreign = qoder_state(&incoming);
        let sql = Database::dump_sql(&*incoming.conn.lock()?, &[])?;
        db.import_sql_string(&sql)?;
        assert_eq!(qoder_receipts::read_rows(&*db.conn.lock()?)?, expected_rows);
        let restored = qoder_state(&db);
        assert_eq!(restored.pending, before.pending);
        assert_ne!(restored.generation, foreign.generation);
        if incoming_key == "synthetic-current" {
            assert_eq!(restored.generation, before.generation);
        } else {
            assert_ne!(restored.generation, before.generation);
        }
        assert_eq!(
            db.get_provider_by_id("qoder-restore-fixture", "codex")?
                .unwrap()
                .settings_config["env"]["OPENAI_API_KEY"],
            incoming_key
        );
    }
    Ok(())
}

#[test]
fn qoder_import_empty_local_owner_never_adopts_foreign_pending() -> Result<(), AppError> {
    let db = Database::memory()?;
    let incoming = same_vault_database(&db)?;
    qoder_fixture(
        &incoming,
        "synthetic-imported",
        "foreign-subject",
        "foreign-attempt",
        100,
    )?;
    let foreign = qoder_state(&incoming);
    let sql = Database::dump_sql(&*incoming.conn.lock()?, &[])?;
    db.import_sql_string(&sql)?;
    assert!(qoder_receipts::read_rows(&*db.conn.lock()?)?.is_empty());
    let restored = qoder_state(&db);
    assert!(restored.pending.is_none());
    assert_ne!(restored.generation, foreign.generation);
    Ok(())
}

#[test]
fn qoder_import_prepare_cannot_erase_or_change_retained_operation() -> Result<(), AppError> {
    let db = Database::memory()?;
    qoder_fixture(
        &db,
        "synthetic-current",
        "local-subject",
        "local-attempt",
        100,
    )?;
    let sql = Database::dump_sql(&*db.conn.lock()?, &[])?;
    let before = Database::content_digest(&*db.conn.lock()?)?;
    for mutation in [
        "DELETE FROM qoder_campaign_operations",
        "UPDATE qoder_campaign_operations SET result='claimed'",
        "DROP TABLE qoder_campaign_operations",
    ] {
        let called = std::cell::Cell::new(false);
        let error = db
            .import_sql_string_from_cc_switch(&sql, &[], |staged| {
                called.set(true);
                staged.conn.lock()?.execute_batch(mutation)?;
                Ok(())
            })
            .expect_err("retained operation mutation must fail before publication");
        assert!(called.get(), "test must reach the actual prepare callback");
        if mutation.starts_with("DROP") {
            assert!(matches!(error, AppError::Database(_)));
        } else {
            assert!(
                matches!(error, AppError::Config(code) if code == "qoder.local_operations_changed")
            );
        }
        assert_eq!(Database::content_digest(&*db.conn.lock()?)?, before);
    }
    Ok(())
}

#[test]
#[serial]
fn qoder_backup_restore_keeps_latest_receipt_and_never_revives_old_generation(
) -> Result<(), AppError> {
    let _home = TestHomeGuard::new();
    let _settings_scope = crate::settings::TestSettingsScope::enter()?;
    let db = crate::secrets::testing::initialize_database()?;
    let _settings = SettingsGuard::with_backup_retain_count(1);
    let old = qoder_fixture(&db, "synthetic-old", "local-subject", "old-attempt", 100)?;
    let old_state = qoder_state(&db);
    let source = db
        .backup_database_file()?
        .expect("real file-backed snapshot");
    let source_name = source.file_name().unwrap().to_string_lossy().into_owned();
    db.finish_qoder_operation(&old, qoder_contract::Outcome::ClaimedDeltaUnknown, None)?;
    let latest = qoder_fixture(
        &db,
        "synthetic-current",
        "local-subject",
        "latest-attempt",
        200,
    )?;
    let current = qoder_state(&db);
    let current_rows = qoder_receipts::read_rows(&*db.conn.lock()?)?;
    let before = Database::content_digest(&*db.conn.lock()?)?;

    let safety_id = db.restore_from_backup(&source_name)?;
    assert_eq!(qoder_receipts::read_rows(&*db.conn.lock()?)?, current_rows);
    let restored = qoder_state(&db);
    assert_ne!(restored.generation, old_state.generation);
    assert_ne!(restored.generation, current.generation);
    assert_eq!(restored.pending.as_ref().unwrap().id, latest.id);
    assert_eq!(
        restored.pending.as_ref().unwrap().generation,
        latest.generation
    );
    assert_eq!(
        db.get_provider_by_id("qoder-restore-fixture", "codex")?
            .unwrap()
            .settings_config["env"]["OPENAI_API_KEY"],
        "synthetic-old"
    );
    assert!(source.exists());
    let safety_path = db
        .secrets
        .root()
        .join("backups")
        .join(format!("{safety_id}.db"));
    let safety =
        Connection::open_with_flags(safety_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    assert_eq!(Database::content_digest(&safety)?, before);
    assert_eq!(qoder_receipts::read_rows(&safety)?, current_rows);
    let vault = db.secrets.read()?;
    let providers = Database::get_all_providers_on_connection(&safety, &vault, "codex")?;
    assert_eq!(
        providers["qoder-restore-fixture"]
            .meta
            .as_ref()
            .unwrap()
            .qoder_campaign_state
            .as_ref()
            .unwrap(),
        &current
    );
    Ok(())
}

#[test]
#[serial]
fn qoder_sync_join_rewraps_provider_but_keeps_only_current_operation_owner() -> Result<(), AppError>
{
    let home = TestHomeGuard::new();
    let current = VaultContext::generate()
        .unwrap()
        .with_password("synthetic-current-password")
        .unwrap();
    let next = VaultContext::generate()
        .unwrap()
        .with_password("synthetic-next-password")
        .unwrap();
    let local = sync_test_database(&current, &home.path().join("current"));
    let incoming = sync_test_database(&next, &home.path().join("incoming"));
    qoder_fixture(
        &local,
        "synthetic-current",
        "local-subject",
        "local-attempt",
        100,
    )?;
    qoder_fixture(
        &incoming,
        "synthetic-changed",
        "foreign-subject",
        "foreign-attempt",
        100,
    )?;
    let local_state = qoder_state(&local);
    let foreign_state = qoder_state(&incoming);
    let expected_rows = qoder_receipts::read_rows(&*local.conn.lock()?)?;
    let before = Database::content_digest(&*local.conn.lock()?)?;
    let (sql, metadata) = incoming.export_sync_snapshot()?;
    let exported = Database::stage_import_sql(&sql, true)?;
    assert!(qoder_receipts::read_rows(&exported)?.is_empty());
    let staged = local.validate_sync_snapshot_for_current(&sql, &metadata, &next)?;
    let joined = Database::prepare_sync_join(staged, &*local.conn.lock()?, &current, &next)?;
    assert_eq!(Database::content_digest(&*local.conn.lock()?)?, before);
    assert_eq!(qoder_receipts::read_rows(&joined)?, expected_rows);
    let providers = Database::get_all_providers_on_connection(&joined, &next, "codex")?;
    let provider = &providers["qoder-restore-fixture"];
    let restored = provider
        .meta
        .as_ref()
        .unwrap()
        .qoder_campaign_state
        .as_ref()
        .unwrap();
    assert_eq!(restored.pending, local_state.pending);
    assert_ne!(restored.generation, local_state.generation);
    assert_ne!(restored.generation, foreign_state.generation);
    assert_eq!(
        provider.settings_config["env"]["OPENAI_API_KEY"],
        "synthetic-changed"
    );
    assert!(Database::get_all_providers_on_connection(&joined, &current, "codex").is_err());
    Ok(())
}
